//! Llama-2 inference for the TinyStories models (karpathy/llama2.c checkpoint format).
//! A from-scratch port of llama2.c's `run.c` (MIT, Andrej Karpathy). Dot products accumulate in
//! f64: exact f32 products, near-exact sums, and no per-op f32 rounding in the Luau output.

pub mod chat;

use std::collections::BTreeMap;

pub struct Config {
    pub dim: usize,
    pub hidden_dim: usize,
    pub n_layers: usize,
    pub n_heads: usize,
    pub n_kv_heads: usize,
    pub vocab_size: usize,
    pub seq_len: usize,
}

struct Weights {
    tok_emb: &'static [f32],
    rms_att: &'static [f32],
    wq: &'static [f32],
    wk: &'static [f32],
    wv: &'static [f32],
    wo: &'static [f32],
    rms_ffn: &'static [f32],
    w1: &'static [f32],
    w2: &'static [f32],
    w3: &'static [f32],
    rms_final: &'static [f32],
    wcls: &'static [f32],
}

struct State {
    x: Vec<f32>,
    xb: Vec<f32>,
    xb2: Vec<f32>,
    hb: Vec<f32>,
    hb2: Vec<f32>,
    q: Vec<f32>,
    att: Vec<f32>,
    logits: Vec<f32>,
    key_cache: Vec<f32>,
    value_cache: Vec<f32>,
}

pub struct Tokenizer {
    vocab: Vec<Vec<u8>>,
    scores: Vec<f32>,
    lookup: BTreeMap<Vec<u8>, u32>,
}

pub struct Model {
    pub cfg: Config,
    w: Weights,
    s: State,
    pub tok: Tokenizer,
    rng: u64,
    pub temperature: f32,
    // generation state
    prompt: Vec<u32>,
    token: u32,
    pub pos: usize,
    pub text: Vec<u8>,
}

fn rmsnorm(o: &mut [f32], x: &[f32], w: &[f32]) {
    let mut ss = 0f64;
    for &v in x {
        ss += v as f64 * v as f64;
    }
    let ss = 1.0 / (ss / x.len() as f64 + 1e-5).sqrt();
    for i in 0..o.len() {
        o[i] = (w[i] as f64 * (ss * x[i] as f64)) as f32;
    }
}

fn softmax(x: &mut [f32]) {
    let mut max = x[0];
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

/// o[d] = W[d x n] @ x[n]
fn matmul(o: &mut [f32], x: &[f32], w: &[f32]) {
    let n = x.len();
    for (i, out) in o.iter_mut().enumerate() {
        let row = &w[i * n..i * n + n];
        let mut v = 0f64;
        for (&a, &b) in row.iter().zip(x) {
            v += a as f64 * b as f64;
        }
        *out = v as f32;
    }
}

impl Model {
    /// `model`/`tokenizer` must stay alive for the program's lifetime (they are leaked buffers).
    pub fn new(model: &'static [u8], tokenizer: &[u8], seed: u64) -> Model {
        let hdr = |i: usize| i32::from_le_bytes(model[i * 4..i * 4 + 4].try_into().unwrap());
        let shared = hdr(5) > 0;
        let cfg = Config {
            dim: hdr(0) as usize,
            hidden_dim: hdr(1) as usize,
            n_layers: hdr(2) as usize,
            n_heads: hdr(3) as usize,
            n_kv_heads: hdr(4) as usize,
            vocab_size: hdr(5).unsigned_abs() as usize,
            seq_len: hdr(6) as usize,
        };
        assert!(model.as_ptr() as usize % 4 == 0, "model buffer must be 4-aligned");
        let floats: &'static [f32] =
            unsafe { core::slice::from_raw_parts(model.as_ptr().add(28) as *const f32, (model.len() - 28) / 4) };
        let mut at = 0;
        let mut take = |n: usize| {
            let s = &floats[at..at + n];
            at += n;
            s
        };
        let (d, h, l, v) = (cfg.dim, cfg.hidden_dim, cfg.n_layers, cfg.vocab_size);
        let head = d / cfg.n_heads;
        let kv = head * cfg.n_kv_heads;
        let tok_emb = take(v * d);
        let rms_att = take(l * d);
        let wq = take(l * d * d);
        let wk = take(l * d * kv);
        let wv = take(l * d * kv);
        let wo = take(l * d * d);
        let rms_ffn = take(l * d);
        let w1 = take(l * h * d);
        let w2 = take(l * d * h);
        let w3 = take(l * h * d);
        let rms_final = take(d);
        take(cfg.seq_len * head); // legacy freq_cis_real + freq_cis_imag
        let wcls = if shared { tok_emb } else { take(v * d) };
        let w = Weights { tok_emb, rms_att, wq, wk, wv, wo, rms_ffn, w1, w2, w3, rms_final, wcls };
        let s = State {
            x: vec![0.0; d],
            xb: vec![0.0; d],
            xb2: vec![0.0; d],
            hb: vec![0.0; h],
            hb2: vec![0.0; h],
            q: vec![0.0; d],
            att: vec![0.0; cfg.n_heads * cfg.seq_len],
            logits: vec![0.0; v],
            key_cache: vec![0.0; l * cfg.seq_len * kv],
            value_cache: vec![0.0; l * cfg.seq_len * kv],
        };
        let tok = Tokenizer::new(tokenizer, v);
        Model { cfg, w, s, tok, rng: seed.max(1), temperature: 0.0, prompt: vec![], token: 1, pos: 0, text: vec![] }
    }

    fn forward(&mut self, token: u32, pos: usize) {
        let c = &self.cfg;
        let (w, s) = (&self.w, &mut self.s);
        let d = c.dim;
        let hd = c.hidden_dim;
        let head = d / c.n_heads;
        let kv_dim = head * c.n_kv_heads;
        let kv_mul = c.n_heads / c.n_kv_heads;
        let t0 = token as usize * d;
        s.x.copy_from_slice(&w.tok_emb[t0..t0 + d]);
        for l in 0..c.n_layers {
            rmsnorm(&mut s.xb, &s.x, &w.rms_att[l * d..(l + 1) * d]);
            let loff = l * c.seq_len * kv_dim;
            let kc = loff + pos * kv_dim;
            matmul(&mut s.q, &s.xb, &w.wq[l * d * d..(l + 1) * d * d]);
            matmul(&mut s.key_cache[kc..kc + kv_dim], &s.xb, &w.wk[l * d * kv_dim..(l + 1) * d * kv_dim]);
            matmul(&mut s.value_cache[kc..kc + kv_dim], &s.xb, &w.wv[l * d * kv_dim..(l + 1) * d * kv_dim]);
            // RoPE
            for i in (0..d).step_by(2) {
                let hdim = (i % head) as f64;
                let freq = 1.0 / 10000f64.powf(hdim / head as f64);
                let val = pos as f64 * freq;
                let (fci, fcr) = val.sin_cos();
                let rot = |v: &mut [f32]| {
                    let (v0, v1) = (v[i] as f64, v[i + 1] as f64);
                    v[i] = (v0 * fcr - v1 * fci) as f32;
                    v[i + 1] = (v0 * fci + v1 * fcr) as f32;
                };
                rot(&mut s.q);
                if i < kv_dim {
                    rot(&mut s.key_cache[kc..kc + kv_dim]);
                }
            }
            // attention
            let scale = 1.0 / (head as f64).sqrt();
            for h in 0..c.n_heads {
                let q = &s.q[h * head..(h + 1) * head];
                let att = &mut s.att[h * c.seq_len..h * c.seq_len + pos + 1];
                let ko = (h / kv_mul) * head;
                for t in 0..=pos {
                    let k = &s.key_cache[loff + t * kv_dim + ko..loff + t * kv_dim + ko + head];
                    let mut score = 0f64;
                    for (&a, &b) in q.iter().zip(k) {
                        score += a as f64 * b as f64;
                    }
                    att[t] = (score * scale) as f32;
                }
                softmax(att);
                let xb = &mut s.xb[h * head..(h + 1) * head];
                for i in 0..head {
                    let mut acc = 0f64;
                    for t in 0..=pos {
                        acc += att[t] as f64 * s.value_cache[loff + t * kv_dim + ko + i] as f64;
                    }
                    xb[i] = acc as f32;
                }
            }
            matmul(&mut s.xb2, &s.xb, &w.wo[l * d * d..(l + 1) * d * d]);
            for i in 0..d {
                s.x[i] += s.xb2[i];
            }
            // FFN
            rmsnorm(&mut s.xb, &s.x, &w.rms_ffn[l * d..(l + 1) * d]);
            matmul(&mut s.hb, &s.xb, &w.w1[l * d * hd..(l + 1) * d * hd]);
            matmul(&mut s.hb2, &s.xb, &w.w3[l * d * hd..(l + 1) * d * hd]);
            for i in 0..hd {
                let v = s.hb[i] as f64;
                s.hb[i] = (v / (1.0 + (-v).exp()) * s.hb2[i] as f64) as f32;
            }
            matmul(&mut s.xb, &s.hb, &w.w2[l * d * hd..(l + 1) * d * hd]);
            for i in 0..d {
                s.x[i] += s.xb[i];
            }
        }
        let x = s.x.clone();
        rmsnorm(&mut s.x, &x, w.rms_final);
        matmul(&mut s.logits, &s.x, w.wcls);
    }

    fn random_f32(&mut self) -> f32 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        ((self.rng.wrapping_mul(0x2545F4914F6CDD1D) >> 32) >> 8) as f32 / 16777216.0
    }

    fn sample(&mut self) -> u32 {
        let logits = &mut self.s.logits;
        if self.temperature <= 0.0 {
            let mut best = 0;
            for i in 1..logits.len() {
                if logits[i] > logits[best] {
                    best = i;
                }
            }
            return best as u32;
        }
        for v in logits.iter_mut() {
            *v /= self.temperature;
        }
        softmax(logits);
        let r = self.random_f32();
        let mut cdf = 0f32;
        let logits = &self.s.logits;
        for (i, &p) in logits.iter().enumerate() {
            cdf += p;
            if r < cdf {
                return i as u32;
            }
        }
        logits.len() as u32 - 1
    }

    /// Start a new story from `prompt` (may be empty).
    pub fn start(&mut self, prompt: &str) {
        self.prompt = self.tok.encode(prompt);
        self.token = 1; // BOS
        self.pos = 0;
        self.text.clear();
    }

    /// Generate one token; appends its text to `self.text` (cleared first). Returns false
    /// when the story has ended (BOS or the context is full).
    pub fn step(&mut self) -> bool {
        self.text.clear();
        if self.pos >= self.cfg.seq_len {
            return false;
        }
        self.forward(self.token, self.pos);
        let next = if self.pos < self.prompt.len() { self.prompt[self.pos] } else { self.sample() };
        self.pos += 1;
        if next == 1 {
            return false;
        }
        let piece = self.tok.decode(self.token, next);
        self.text.extend_from_slice(piece);
        self.token = next;
        true
    }
}

impl Tokenizer {
    fn new(b: &[u8], vocab_size: usize) -> Tokenizer {
        let mut at = 4; // max_token_length
        let mut vocab = Vec::with_capacity(vocab_size);
        let mut scores = Vec::with_capacity(vocab_size);
        let mut lookup = BTreeMap::new();
        for i in 0..vocab_size {
            let score = f32::from_le_bytes(b[at..at + 4].try_into().unwrap());
            let len = i32::from_le_bytes(b[at + 4..at + 8].try_into().unwrap()) as usize;
            let s = b[at + 8..at + 8 + len].to_vec();
            at += 8 + len;
            lookup.entry(s.clone()).or_insert(i as u32);
            vocab.push(s);
            scores.push(score);
        }
        Tokenizer { vocab, scores, lookup }
    }

    fn decode(&self, prev: u32, token: u32) -> &[u8] {
        let mut p = &self.vocab[token as usize][..];
        if prev == 1 && p.first() == Some(&b' ') {
            p = &p[1..];
        }
        // raw byte tokens "<0xAB>"
        if p.len() == 6 && p.starts_with(b"<0x") && p[5] == b'>' {
            if let Ok(s) = core::str::from_utf8(&p[3..5]) {
                if let Ok(v) = u8::from_str_radix(s, 16) {
                    return &BYTES[v as usize..v as usize + 1];
                }
            }
        }
        p
    }

    /// SentencePiece-style BPE: start from single bytes, greedily merge the best-scoring pair.
    fn encode(&self, text: &str) -> Vec<u32> {
        let mut toks: Vec<u32> = vec![];
        if text.is_empty() {
            return toks;
        }
        if let Some(&t) = self.lookup.get(&b" "[..]) {
            toks.push(t); // dummy prefix space
        }
        for ch in text.chars() {
            let mut buf = [0u8; 4];
            let s = ch.encode_utf8(&mut buf).as_bytes();
            match self.lookup.get(s) {
                Some(&t) => toks.push(t),
                None => toks.extend(s.iter().map(|&b| b as u32 + 3)), // byte fallback
            }
        }
        loop {
            let mut best: Option<(f32, usize, u32)> = None;
            for i in 0..toks.len().saturating_sub(1) {
                let mut m = self.vocab[toks[i] as usize].clone();
                m.extend_from_slice(&self.vocab[toks[i + 1] as usize]);
                if let Some(&id) = self.lookup.get(&m) {
                    let sc = self.scores[id as usize];
                    if best.map_or(true, |b| sc > b.0) {
                        best = Some((sc, i, id));
                    }
                }
            }
            let Some((_, i, id)) = best else { break };
            toks[i] = id;
            toks.remove(i + 1);
        }
        toks
    }
}

static BYTES: [u8; 256] = {
    let mut b = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        b[i] = i as u8;
        i += 1;
    }
    b
};

// ---- Luau exports -------------------------------------------------------------------------

static mut MODEL: Option<Model> = None;

#[allow(static_mut_refs)]
fn model() -> &'static mut Model {
    unsafe { MODEL.as_mut().expect("model not loaded") }
}

/// Allocate `len` bytes (4-aligned) for the caller to fill; returns the address.
#[unsafe(no_mangle)]
pub extern "C" fn llm_alloc(len: u32) -> u32 {
    let v = vec![0u32; (len as usize).div_ceil(4)].leak();
    v.as_mut_ptr() as usize as u32
}

#[unsafe(no_mangle)]
pub extern "C" fn llm_init(model_ptr: u32, model_len: u32, tok_ptr: u32, tok_len: u32, seed: f64) {
    let m = unsafe { core::slice::from_raw_parts(model_ptr as usize as *const u8, model_len as usize) };
    let t = unsafe { core::slice::from_raw_parts(tok_ptr as usize as *const u8, tok_len as usize) };
    unsafe { MODEL = Some(Model::new(m, t, seed as u64)) };
}

#[unsafe(no_mangle)]
pub extern "C" fn llm_set_temperature(t: f64) {
    model().temperature = t as f32;
}

/// Start a story from the UTF-8 prompt at `ptr..ptr+len` (written into an `llm_alloc` buffer).
#[unsafe(no_mangle)]
pub extern "C" fn llm_start(ptr: u32, len: u32) {
    let p = unsafe { core::slice::from_raw_parts(ptr as usize as *const u8, len as usize) };
    model().start(core::str::from_utf8(p).unwrap_or(""));
}

/// Generate one token. Returns the byte length of its text (at `llm_text()`), or -1 at the end.
#[unsafe(no_mangle)]
pub extern "C" fn llm_step() -> i32 {
    let m = model();
    if m.step() { m.text.len() as i32 } else { -1 }
}

#[unsafe(no_mangle)]
pub extern "C" fn llm_text() -> u32 {
    model().text.as_ptr() as usize as u32
}
