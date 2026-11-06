//! Chat model inference (SmolLM2-style Llama: GQA, non-interleaved RoPE, tied embeddings) over
//! Q8 weights (int8 + one f32 scale per 64 values), with a GPT-2 byte-level BPE tokenizer and
//! the ChatML template. File formats are produced by `tools/convert_smol.py`.

use std::collections::BTreeMap;

const GS: usize = 64;

/// Q8 matrix [rows x cols], row-major; scales per group of GS along each row.
struct Q8 {
    scales: &'static [f32],
    q: &'static [i8],
}

struct Layer {
    attn_norm: &'static [f32],
    wq: Q8,
    wk: Q8,
    wv: Q8,
    wo: Q8,
    ffn_norm: &'static [f32],
    w_gate: Q8,
    w_down: Q8,
    w_up: Q8,
}

pub struct Chat {
    dim: usize,
    hidden: usize,
    n_heads: usize,
    n_kv: usize,
    head: usize,
    vocab: usize,
    pub seq_len: usize,
    eps: f64,
    inv_freq: Vec<f64>,
    embed: Q8,
    layers: Vec<Layer>,
    final_norm: &'static [f32],
    // state
    x: Vec<f32>,
    xb: Vec<f32>,
    xb2: Vec<f32>,
    hb: Vec<f32>,
    hb2: Vec<f32>,
    q: Vec<f32>,
    att: Vec<f32>,
    logits: Vec<f32>,
    kc: Vec<f32>,
    vc: Vec<f32>,
    // tokenizer
    pieces: Vec<Vec<u8>>,
    byte_tok: [i32; 256],
    merges: BTreeMap<(u32, u32), (u32, u32)>, // (a, b) -> (rank, result)
    // conversation
    pub pos: usize,
    pending: Vec<u32>,
    last: u32,
    recent: Vec<u32>,
    rng: u64,
    pub temperature: f32,
    pub top_k: usize,
    started: bool,
    carry: bool,
    pub text: Vec<u8>,
}

const IM_START: u32 = 1;
const IM_END: u32 = 2;
const EOT: u32 = 0;

fn rmsnorm(o: &mut [f32], x: &[f32], w: &[f32], eps: f64) {
    let mut ss = 0f64;
    for &v in x {
        ss += v as f64 * v as f64;
    }
    let ss = 1.0 / (ss / x.len() as f64 + eps).sqrt();
    for ((o, &x), &w) in o.iter_mut().zip(x).zip(w) {
        *o = (w as f64 * (ss * x as f64)) as f32;
    }
}

fn softmax(x: &mut [f32]) {
    let mut max = f32::NEG_INFINITY;
    for &v in x.iter() {
        if v > max {
            max = v;
        }
    }
    let mut sum = 0f64;
    for v in x.iter_mut() {
        let e = ((*v - max) as f64).exp();
        *v = e as f32;
        sum += e;
    }
    for v in x.iter_mut() {
        *v = (*v as f64 / sum) as f32;
    }
}

/// o[r] = W[r, :] . x
fn qmatmul(o: &mut [f32], x: &[f32], w: &Q8) {
    let n = x.len();
    let ng = n / GS;
    for (r, out) in o.iter_mut().enumerate() {
        let row = &w.q[r * n..r * n + n];
        let sc = &w.scales[r * ng..r * ng + ng];
        let mut acc = 0f64;
        for ((qg, xg), &s) in row.chunks_exact(GS).zip(x.chunks_exact(GS)).zip(sc) {
            // unrolled by 8 over fixed-size arrays: no bounds checks, 1/8 of the loop overhead
            let mut g = 0f64;
            for (q, x) in qg.as_chunks::<16>().0.iter().zip(xg.as_chunks::<16>().0) {
                g += q[0] as f64 * x[0] as f64 + q[1] as f64 * x[1] as f64 + q[2] as f64 * x[2] as f64 + q[3] as f64 * x[3] as f64 + q[4] as f64 * x[4] as f64 + q[5] as f64 * x[5] as f64 + q[6] as f64 * x[6] as f64 + q[7] as f64 * x[7] as f64 + q[8] as f64 * x[8] as f64 + q[9] as f64 * x[9] as f64 + q[10] as f64 * x[10] as f64 + q[11] as f64 * x[11] as f64 + q[12] as f64 * x[12] as f64 + q[13] as f64 * x[13] as f64 + q[14] as f64 * x[14] as f64 + q[15] as f64 * x[15] as f64;
            }
            acc += g * s as f64;
        }
        *out = acc as f32;
    }
}

struct Reader {
    b: &'static [u8],
    at: usize,
}
impl Reader {
    fn i32(&mut self) -> i32 {
        let v = i32::from_le_bytes(self.b[self.at..self.at + 4].try_into().unwrap());
        self.at += 4;
        v
    }
    fn f32s(&mut self, n: usize) -> &'static [f32] {
        assert!((self.b.as_ptr() as usize + self.at) % 4 == 0);
        let s = unsafe { core::slice::from_raw_parts(self.b.as_ptr().add(self.at) as *const f32, n) };
        self.at += n * 4;
        s
    }
    fn q8(&mut self, rows: usize, cols: usize) -> Q8 {
        let n = rows * cols;
        let scales = self.f32s(n / GS);
        let q = unsafe { core::slice::from_raw_parts(self.b.as_ptr().add(self.at) as *const i8, n) };
        self.at += n;
        Q8 { scales, q }
    }
}

impl Chat {
    pub fn new(model: &'static [u8], tok: &[u8], seq_len: usize, seed: u64) -> Chat {
        assert!(&model[..4] == b"SMQ8", "not a Q8 chat model");
        let mut r = Reader { b: model, at: 4 };
        let (dim, hidden, nl, n_heads, n_kv, vocab, gs) =
            (r.i32() as usize, r.i32() as usize, r.i32() as usize, r.i32() as usize, r.i32() as usize, r.i32() as usize, r.i32() as usize);
        assert_eq!(gs, GS);
        let theta = f32::from_bits(r.i32() as u32) as f64;
        let eps = f32::from_bits(r.i32() as u32) as f64;
        let head = dim / n_heads;
        assert!(head % 16 == 0 && dim % GS == 0 && hidden % GS == 0, "unsupported model shape");
        let kv_dim = head * n_kv;
        let embed = r.q8(vocab, dim);
        let mut layers = Vec::with_capacity(nl);
        for _ in 0..nl {
            layers.push(Layer {
                attn_norm: r.f32s(dim),
                wq: r.q8(dim, dim),
                wk: r.q8(kv_dim, dim),
                wv: r.q8(kv_dim, dim),
                wo: r.q8(dim, dim),
                ffn_norm: r.f32s(dim),
                w_gate: r.q8(hidden, dim),
                w_down: r.q8(dim, hidden),
                w_up: r.q8(hidden, dim),
            });
        }
        let final_norm = r.f32s(dim);
        let inv_freq = (0..head / 2).map(|i| 1.0 / theta.powf(2.0 * i as f64 / head as f64)).collect();

        // tokenizer
        let rd = |at: usize| i32::from_le_bytes(tok[at..at + 4].try_into().unwrap());
        let (tv, nm) = (rd(0) as usize, rd(4) as usize);
        let mut at = 12;
        let mut byte_tok = [0i32; 256];
        for b in byte_tok.iter_mut() {
            *b = rd(at);
            at += 4;
        }
        let mut pieces = Vec::with_capacity(tv);
        for _ in 0..tv {
            let len = u16::from_le_bytes([tok[at], tok[at + 1]]) as usize;
            pieces.push(tok[at + 2..at + 2 + len].to_vec());
            at += 2 + len;
        }
        let mut merges = BTreeMap::new();
        for rank in 0..nm {
            let (a, b, res) = (rd(at) as u32, rd(at + 4) as u32, rd(at + 8) as u32);
            at += 12;
            merges.entry((a, b)).or_insert((rank as u32, res));
        }

        Chat {
            dim,
            hidden,
            n_heads,
            n_kv,
            head,
            vocab,
            seq_len,
            eps,
            inv_freq,
            embed,
            layers,
            final_norm,
            x: vec![0.0; dim],
            xb: vec![0.0; dim],
            xb2: vec![0.0; dim],
            hb: vec![0.0; hidden],
            hb2: vec![0.0; hidden],
            q: vec![0.0; dim],
            att: vec![0.0; seq_len],
            logits: vec![0.0; vocab],
            kc: vec![0.0; nl * seq_len * kv_dim],
            vc: vec![0.0; nl * seq_len * kv_dim],
            pieces,
            byte_tok,
            merges,
            pos: 0,
            pending: vec![],
            last: 0,
            recent: vec![],
            rng: seed.max(1),
            temperature: 0.7,
            top_k: 40,
            started: false,
            carry: false,
            text: vec![],
        }
    }

    fn rope(v: &mut [f32], inv_freq: &[f64], pos: usize, head: usize) {
        let half = head / 2;
        for h in v.chunks_exact_mut(head) {
            for i in 0..half {
                let (s, c) = (pos as f64 * inv_freq[i]).sin_cos();
                let (a, b) = (h[i] as f64, h[i + half] as f64);
                h[i] = (a * c - b * s) as f32;
                h[i + half] = (b * c + a * s) as f32;
            }
        }
    }

    fn forward(&mut self, token: u32, pos: usize) {
        let (d, head) = (self.dim, self.head);
        let kv_dim = head * self.n_kv;
        let kv_mul = self.n_heads / self.n_kv;
        // embedding row (dequantized)
        let ng = d / GS;
        let t = token as usize;
        for g in 0..ng {
            let s = self.embed.scales[t * ng + g];
            for j in 0..GS {
                self.x[g * GS + j] = self.embed.q[t * d + g * GS + j] as f32 * s;
            }
        }
        let scale = 1.0 / (head as f64).sqrt();
        for (l, ly) in self.layers.iter().enumerate() {
            rmsnorm(&mut self.xb, &self.x, ly.attn_norm, self.eps);
            let loff = l * self.seq_len * kv_dim;
            let kc0 = loff + pos * kv_dim;
            qmatmul(&mut self.q, &self.xb, &ly.wq);
            qmatmul(&mut self.kc[kc0..kc0 + kv_dim], &self.xb, &ly.wk);
            qmatmul(&mut self.vc[kc0..kc0 + kv_dim], &self.xb, &ly.wv);
            Self::rope(&mut self.q, &self.inv_freq, pos, head);
            Self::rope(&mut self.kc[kc0..kc0 + kv_dim], &self.inv_freq, pos, head);
            for h in 0..self.n_heads {
                let q = &self.q[h * head..(h + 1) * head];
                let ko = (h / kv_mul) * head;
                let att = &mut self.att[..pos + 1];
                for (t, a) in att.iter_mut().enumerate() {
                    let k = &self.kc[loff + t * kv_dim + ko..loff + t * kv_dim + ko + head];
                    let mut s = 0f64;
                    for (x, y) in q.as_chunks::<16>().0.iter().zip(k.as_chunks::<16>().0) {
                        s += x[0] as f64 * y[0] as f64 + x[1] as f64 * y[1] as f64 + x[2] as f64 * y[2] as f64 + x[3] as f64 * y[3] as f64 + x[4] as f64 * y[4] as f64 + x[5] as f64 * y[5] as f64 + x[6] as f64 * y[6] as f64 + x[7] as f64 * y[7] as f64 + x[8] as f64 * y[8] as f64 + x[9] as f64 * y[9] as f64 + x[10] as f64 * y[10] as f64 + x[11] as f64 * y[11] as f64 + x[12] as f64 * y[12] as f64 + x[13] as f64 * y[13] as f64 + x[14] as f64 * y[14] as f64 + x[15] as f64 * y[15] as f64;
                    }
                    *a = (s * scale) as f32;
                }
                softmax(att);
                let xb = &mut self.xb[h * head..(h + 1) * head];
                for v in xb.iter_mut() {
                    *v = 0.0;
                }
                let mut acc = vec![0f64; 0];
                acc.resize(head, 0.0);
                for (t, &a) in att.iter().enumerate() {
                    let vrow = &self.vc[loff + t * kv_dim + ko..loff + t * kv_dim + ko + head];
                    let a = a as f64;
                    for (o, v) in acc.as_chunks_mut::<16>().0.iter_mut().zip(vrow.as_chunks::<16>().0) {
                        o[0] += a * v[0] as f64;
                        o[1] += a * v[1] as f64;
                        o[2] += a * v[2] as f64;
                        o[3] += a * v[3] as f64;
                        o[4] += a * v[4] as f64;
                        o[5] += a * v[5] as f64;
                        o[6] += a * v[6] as f64;
                        o[7] += a * v[7] as f64;
                        o[8] += a * v[8] as f64;
                        o[9] += a * v[9] as f64;
                        o[10] += a * v[10] as f64;
                        o[11] += a * v[11] as f64;
                        o[12] += a * v[12] as f64;
                        o[13] += a * v[13] as f64;
                        o[14] += a * v[14] as f64;
                        o[15] += a * v[15] as f64;
                    }
                }
                for (o, a) in xb.iter_mut().zip(acc) {
                    *o = a as f32;
                }
            }
            qmatmul(&mut self.xb2, &self.xb, &ly.wo);
            for (x, &y) in self.x.iter_mut().zip(&self.xb2) {
                *x += y;
            }
            rmsnorm(&mut self.xb, &self.x, ly.ffn_norm, self.eps);
            qmatmul(&mut self.hb, &self.xb, &ly.w_gate);
            qmatmul(&mut self.hb2, &self.xb, &ly.w_up);
            for (g, &u) in self.hb.iter_mut().zip(&self.hb2) {
                let v = *g as f64;
                *g = (v / (1.0 + (-v).exp()) * u as f64) as f32;
            }
            qmatmul(&mut self.xb, &self.hb, &ly.w_down);
            for (x, &y) in self.x.iter_mut().zip(&self.xb) {
                *x += y;
            }
        }
        let x = self.x.clone();
        rmsnorm(&mut self.x, &x, self.final_norm, self.eps);
        let _ = self.hidden;
        qmatmul(&mut self.logits, &self.x, &self.embed);
    }

    fn random(&mut self) -> f64 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545F4914F6CDD1D) >> 11) as f64 / (1u64 << 53) as f64
    }

    fn sample(&mut self) -> u32 {
        // repetition penalty over recent output
        for &t in &self.recent {
            let l = &mut self.logits[t as usize];
            *l = if *l > 0.0 { *l / 1.15 } else { *l * 1.15 };
        }
        if self.temperature <= 0.0 {
            let mut best = 0;
            for i in 1..self.vocab {
                if self.logits[i] > self.logits[best] {
                    best = i;
                }
            }
            return best as u32;
        }
        // top-k by insertion into a small sorted list
        let k = self.top_k.max(1);
        let mut top: Vec<(f32, u32)> = Vec::with_capacity(k + 1);
        for (i, &l) in self.logits.iter().enumerate() {
            if top.len() == k && l <= top[k - 1].0 {
                continue;
            }
            let at = top.iter().position(|e| l > e.0).unwrap_or(top.len());
            top.insert(at, (l, i as u32));
            top.truncate(k);
        }
        let mut probs: Vec<f32> = top.iter().map(|e| e.0 / self.temperature).collect();
        softmax(&mut probs);
        let r = self.random();
        let mut cdf = 0f64;
        for (p, e) in probs.iter().zip(&top) {
            cdf += *p as f64;
            if r < cdf {
                return e.1;
            }
        }
        top.last().unwrap().1
    }

    /// GPT-2 style pre-tokenization (digits split individually) then byte-level BPE.
    pub fn encode(&self, text: &str) -> Vec<u32> {
        let mut out = vec![];
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        let cls = |c: char| {
            if c.is_ascii_digit() || c.is_numeric() {
                2
            } else if c.is_alphabetic() {
                1
            } else if c.is_whitespace() {
                3
            } else {
                4
            }
        };
        while i < chars.len() {
            let start = i;
            let c = chars[i];
            if c == '\'' && i + 1 < chars.len() {
                let rest: String = chars[i + 1..chars.len().min(i + 3)].iter().collect();
                let n = ["re", "ve", "ll"].iter().find(|s| rest.starts_with(**s)).map(|_| 3)
                    .or_else(|| ["s", "t", "m", "d"].iter().find(|s| rest.starts_with(**s)).map(|_| 2));
                if let Some(n) = n {
                    i += n;
                    self.bpe(&chars[start..i], &mut out);
                    continue;
                }
            }
            let mut j = i;
            if c == ' ' && i + 1 < chars.len() && cls(chars[i + 1]) != 3 {
                j += 1; // optional leading space joins the next run
            }
            let k = cls(chars[j]);
            match k {
                2 => j += 1, // one digit at a time
                1 | 4 => {
                    while j < chars.len() && cls(chars[j]) == k && !(k == 4 && chars[j] == '\'' && j > i) {
                        j += 1;
                    }
                }
                _ => {
                    // whitespace run; leave the last one to prefix a following word
                    while j < chars.len() && cls(chars[j]) == 3 {
                        j += 1;
                    }
                    if j < chars.len() && j - i > 1 {
                        j -= 1;
                    }
                }
            }
            if k == 2 && j - i == 2 {
                // " 5": the space is its own token (digits pre-split before the byte-level regex)
                self.bpe(&chars[i..i + 1], &mut out);
                self.bpe(&chars[i + 1..j], &mut out);
            } else {
                self.bpe(&chars[i..j], &mut out);
            }
            i = j;
        }
        out
    }

    fn bpe(&self, chars: &[char], out: &mut Vec<u32>) {
        let s: String = chars.iter().collect();
        let mut toks: Vec<u32> = s.bytes().filter_map(|b| u32::try_from(self.byte_tok[b as usize]).ok()).collect();
        loop {
            let mut best: Option<(u32, usize, u32)> = None;
            for i in 0..toks.len().saturating_sub(1) {
                if let Some(&(rank, res)) = self.merges.get(&(toks[i], toks[i + 1])) {
                    if best.map_or(true, |b| rank < b.0) {
                        best = Some((rank, i, res));
                    }
                }
            }
            let Some((_, i, res)) = best else { break };
            toks[i] = res;
            toks.remove(i + 1);
        }
        out.extend(toks);
    }

    /// Queue a user message (ChatML); the assistant's reply follows from `step`.
    pub fn user(&mut self, msg: &str) {
        let mut t = vec![];
        if self.carry {
            t.push(IM_END);
            t.extend(self.encode("\n"));
            self.carry = false;
        }
        if !self.started {
            t.push(IM_START);
            t.extend(self.encode("system\nYou are a helpful AI assistant named SmolLM, trained by Hugging Face"));
            t.push(IM_END);
            t.extend(self.encode("\n"));
            self.started = true;
        }
        t.push(IM_START);
        t.extend(self.encode("user\n"));
        t.extend(self.encode(msg));
        t.push(IM_END);
        t.extend(self.encode("\n"));
        t.push(IM_START);
        t.extend(self.encode("assistant\n"));
        self.pending = t;
        self.pending.reverse();
        self.recent.clear();
    }

    /// Forget the conversation.
    pub fn reset(&mut self) {
        self.pos = 0;
        self.pending.clear();
        self.started = false;
        self.carry = false;
        self.recent.clear();
    }

    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// One forward pass. Returns Some(bytes) of new text (empty while reading the prompt), or
    /// None when the reply has ended (or the context is full).
    pub fn step(&mut self) -> Option<usize> {
        self.text.clear();
        if self.pos >= self.seq_len {
            return None;
        }
        if let Some(t) = self.pending.pop() {
            self.forward(t, self.pos);
            self.pos += 1;
            self.last = t;
            if self.pending.is_empty() {
                // the prompt is in; sample the first reply token from these logits
                return self.emit();
            }
            return Some(0);
        }
        self.forward(self.last, self.pos);
        self.pos += 1;
        self.emit()
    }

    fn emit(&mut self) -> Option<usize> {
        let next = self.sample();
        self.last = next;
        if next == IM_END || next == EOT {
            // the sampled end marker hasn't been through `forward`: it opens the next turn
            self.carry = true;
            return None;
        }
        self.recent.push(next);
        if self.recent.len() > 64 {
            self.recent.remove(0);
        }
        self.text.extend_from_slice(&self.pieces[next as usize]);
        Some(self.text.len())
    }
}

// ---- Luau exports -------------------------------------------------------------------------

static mut CHAT: Option<Chat> = None;

#[allow(static_mut_refs)]
fn chat() -> &'static mut Chat {
    unsafe { CHAT.as_mut().expect("chat model not loaded") }
}

#[unsafe(no_mangle)]
pub extern "C" fn chat_init(model_ptr: u32, model_len: u32, tok_ptr: u32, tok_len: u32, seq_len: u32, seed: f64) {
    let m = unsafe { core::slice::from_raw_parts(model_ptr as usize as *const u8, model_len as usize) };
    let t = unsafe { core::slice::from_raw_parts(tok_ptr as usize as *const u8, tok_len as usize) };
    unsafe { CHAT = Some(Chat::new(m, t, seq_len as usize, seed as u64)) };
}

/// Queue a user message (UTF-8 at `ptr..ptr+len`).
#[unsafe(no_mangle)]
pub extern "C" fn chat_user(ptr: u32, len: u32) {
    let p = unsafe { core::slice::from_raw_parts(ptr as usize as *const u8, len as usize) };
    chat().user(&String::from_utf8_lossy(p));
}

/// One forward pass: >= 0 bytes of reply text at `chat_text()` (0 while reading the prompt),
/// -1 when the reply is complete, -2 when the context is full.
#[unsafe(no_mangle)]
pub extern "C" fn chat_step() -> i32 {
    let c = chat();
    if c.pos >= c.seq_len {
        return -2;
    }
    match c.step() {
        Some(n) => n as i32,
        None => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn chat_text() -> u32 {
    chat().text.as_ptr() as usize as u32
}

/// Prompt tokens still to be read before the reply starts.
#[unsafe(no_mangle)]
pub extern "C" fn chat_pending() -> u32 {
    chat().pending() as u32
}

/// Tokens used in the context so far.
#[unsafe(no_mangle)]
pub extern "C" fn chat_pos() -> u32 {
    chat().pos as u32
}

#[unsafe(no_mangle)]
pub extern "C" fn chat_set_sampling(temperature: f64, top_k: u32) {
    let c = chat();
    c.temperature = temperature as f32;
    c.top_k = top_k as usize;
}

#[unsafe(no_mangle)]
pub extern "C" fn chat_reset() {
    chat().reset();
}
