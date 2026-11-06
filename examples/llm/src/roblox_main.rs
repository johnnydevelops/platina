//! The in-game chat, in Rust. A server Script calls `require(LLM).server()` and a LocalScript
//! calls `require(LLM).client()`; this file does everything else: loading the zstd-compressed
//! weights from `ServerStorage.Model` (see make_place.py), the RemoteEvents, generation, and
//! the chat UI.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use roblox::prelude::*;

use crate::chat::Chat;

const CONTEXT: usize = 1024;

/// Decode chunk modules `prefix001..` (base64 of zstd frames) into one 4-aligned allocation.
fn load_chunks(folder: &Instance, prefix: &str, parts: u32, size: usize, what: &str, status: &dyn Fn(&str)) -> &'static [u8] {
    let words: &'static mut [u32] = vec![0u32; size.div_ceil(4)].leak();
    let bytes = unsafe { std::slice::from_raw_parts_mut(words.as_mut_ptr() as *mut u8, size) };
    let encoding = EncodingService::get();
    let fromstring = Value::global("buffer").get("fromstring");
    let (mut at, t0) = (0usize, task::clock());
    for i in 1..=parts {
        let module = folder.wait_for_child(&format!("{prefix}{i:03}"), 60.0).expect("missing model chunk");
        // require a throwaway copy: require() caches results, which would keep every chunk's
        // base64 string alive
        let copy = module.clone_instance().expect("clone");
        let b64 = Value::require(&copy);
        copy.destroy();
        let packed = encoding.base64_decode(fromstring.call(&[b64]).index(1));
        let raw = encoding.decompress_buffer(packed, enums::CompressionAlgorithm::Zstd);
        let n = raw.len() as usize;
        raw.read_into(&mut bytes[at..at + n]);
        at += n;
        status(&format!("loading {what} {i}/{parts} ({:.0}s)", task::clock() - t0));
        task::wait(0.0);
    }
    assert_eq!(at, size, "size mismatch while loading {what}");
    bytes
}

#[unsafe(no_mangle)]
pub extern "C" fn server() {
    let storage = ReplicatedStorage::get();
    let remotes = Folder::new();
    remotes.set_name("ChatRemotes");
    let ask = RemoteEvent::new();
    ask.set_name("Ask");
    ask.set_parent(Some(&remotes));
    let token = RemoteEvent::new(); // (kind, text): "text" | "status" | "done" | "user"
    token.set_name("Token");
    token.set_parent(Some(&remotes));
    remotes.set_parent(Some(&storage));

    let fire = {
        let token = token.clone();
        move |kind: &str, text: Value| token.fire_all_clients(&[kind.into_value(), text])
    };
    let status = |s: &str| {
        println!("[chat] {s}");
        fire("status", s.into_value());
    };

    task::wait(10.0); // let the first-load hitch settle

    let folder = ServerStorage::get().wait_for_child("Model", 60.0).expect("ServerStorage.Model");
    let attr = |name: &str| folder.get_attribute(name).to_f64();
    let model = load_chunks(&folder, "M", attr("ModelParts") as u32, attr("ModelSize") as usize, "model", &status);
    let tok = load_chunks(&folder, "T", attr("TokParts") as u32, attr("TokSize") as usize, "tokenizer", &status);
    let mut chat = Chat::new(model, tok, CONTEXT, (task::clock() * 1e6) as u64);
    chat.temperature = 0.7;
    chat.top_k = 40;
    status("ready: ask me something");

    let queue: Rc<RefCell<VecDeque<(String, String)>>> = Rc::default();
    let q = queue.clone();
    ask.on_server_event().connect(move |player: Option<Player>, args: Varargs| {
        let Some(text) = args.first().map(|v| v.to_string_luau()) else { return };
        if !text.is_empty() && text.len() <= 500 {
            let name = player.map(|p| p.display_name()).unwrap_or_default();
            q.borrow_mut().push_back((name, text));
        }
    });

    loop {
        let Some((name, text)) = queue.borrow_mut().pop_front() else {
            task::wait(0.1);
            continue;
        };
        fire("user", format!("{name}: {text}").into_value());
        chat.user(&text);
        let total = chat.pending();
        let (t0, mut n) = (task::clock(), 0);
        loop {
            if chat.pos >= chat.seq_len {
                chat.reset();
                status("context full: conversation reset");
                break;
            }
            let Some(len) = chat.step() else { break };
            let pending = chat.pending();
            if pending > 0 {
                status(&format!("reading prompt {}/{total}", total - pending));
            } else if len > 0 {
                n += 1;
                fire("text", Value::string_from_bytes(&chat.text));
            }
            task::wait(0.0);
        }
        fire("done", "".into_value());
        let dt = task::clock() - t0;
        status(&format!("{n} tokens in {dt:.0}s ({:.2} tok/s), context {}/{CONTEXT}", n as f64 / dt, chat.pos));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn client() {
    let player = Players::get().local_player().expect("client() must run in a LocalScript");
    let gui = ScreenGui::new();
    gui.set_name("LLMChat");
    gui.set_reset_on_spawn(false);
    gui.set_ignore_gui_inset(true);
    let bg = Frame::new();
    bg.set_size(UDim2::from_scale(1.0, 1.0));
    bg.set_background_color3(Color3::from_rgb(24, 24, 28));
    bg.set_parent(Some(&gui));

    let status = TextLabel::new();
    status.set_size(UDim2::new(1.0, -32.0, 0.0, 28.0));
    status.set_position(UDim2::from_offset(16.0, 8.0));
    status.set_background_transparency(1.0);
    status.set_text_color3(Color3::from_rgb(150, 150, 160));
    status.set_font(enums::Font::Code);
    status.set_text_size(16.0);
    status.set_text_x_alignment(enums::TextXAlignment::Left);
    status.set_text("waiting for the server...");
    status.set_parent(Some(&bg));

    let log = ScrollingFrame::new();
    log.set_size(UDim2::new(1.0, -32.0, 1.0, -100.0));
    log.set_position(UDim2::from_offset(16.0, 40.0));
    log.set_background_transparency(1.0);
    log.set_border_size_pixel(0);
    log.set_scroll_bar_thickness(6);
    log.set_automatic_canvas_size(enums::AutomaticSize::Y);
    log.set_canvas_size(UDim2::default());
    log.set_parent(Some(&bg));
    let layout = UIListLayout::new();
    layout.set_padding(UDim::new(0.0, 10.0));
    layout.set_sort_order(enums::SortOrder::LayoutOrder);
    layout.set_parent(Some(&log));

    let input = TextBox::new();
    input.set_size(UDim2::new(1.0, -32.0, 0.0, 40.0));
    input.set_position(UDim2::new(0.0, 16.0, 1.0, -52.0));
    input.set_background_color3(Color3::from_rgb(40, 40, 48));
    input.set_text_color3(Color3::new(1.0, 1.0, 1.0));
    input.set_placeholder_text("Message SmolLM (Enter to send)");
    input.set_placeholder_color3(Color3::from_rgb(120, 120, 130));
    input.set_font(enums::Font::Gotham);
    input.set_text_size(18.0);
    input.set_clear_text_on_focus(false);
    input.set_text("");
    input.set_text_x_alignment(enums::TextXAlignment::Left);
    input.set_parent(Some(&bg));
    UICorner::new().set_parent(Some(&input));
    let pad = UIPadding::new();
    pad.set_padding_left(UDim::new(0.0, 10.0));
    pad.set_parent(Some(&input));

    gui.set_parent(Some(&player.wait_for_child("PlayerGui", 60.0).expect("PlayerGui")));

    // chat log: one TextLabel per message; the reply's bytes accumulate as they stream in
    let order = Rc::new(RefCell::new(0));
    let bubble = {
        let log = log.clone();
        move |text: &str, user: bool| -> TextLabel {
            *order.borrow_mut() += 1;
            let t = TextLabel::new();
            t.set_layout_order(*order.borrow());
            t.set_size(UDim2::new(1.0, -12.0, 0.0, 0.0));
            t.set_automatic_size(enums::AutomaticSize::Y);
            t.set_background_transparency(1.0);
            t.set_text_wrapped(true);
            t.set_rich_text(false);
            t.set_font(enums::Font::Gotham);
            t.set_text_size(18.0);
            t.set_text_x_alignment(enums::TextXAlignment::Left);
            t.set_text_color3(if user { Color3::from_rgb(130, 180, 255) } else { Color3::from_rgb(235, 235, 240) });
            t.set_text(text);
            t.set_parent(Some(&log));
            t
        }
    };

    let remotes = ReplicatedStorage::get().wait_for_child("ChatRemotes", 120.0).expect("ChatRemotes");
    let ask: RemoteEvent = remotes.wait_for_child("Ask", 60.0).and_then(|r| r.cast()).expect("Ask");
    let token: RemoteEvent = remotes.wait_for_child("Token", 60.0).and_then(|r| r.cast()).expect("Token");

    let current: Rc<RefCell<Option<(TextLabel, Vec<u8>)>>> = Rc::default();
    token.on_client_event().connect(move |args: Varargs| {
        let kind = args.first().map(|v| v.to_string_luau()).unwrap_or_default();
        let text = args.get(1).cloned().unwrap_or(Value::NIL);
        match kind.as_str() {
            "status" => status.set_text(&text.to_string_luau()),
            "user" => {
                bubble(&text.to_string_luau(), true);
                *current.borrow_mut() = Some((bubble("SmolLM: ...", false), Vec::new()));
            }
            "text" => {
                if let Some((label, raw)) = current.borrow_mut().as_mut() {
                    raw.extend_from_slice(&text.to_bytes());
                    label.set_text(&format!("SmolLM: {}", String::from_utf8_lossy(raw)));
                    log.set_canvas_position(Vector2::new(0.0, 1e9));
                }
            }
            "done" => *current.borrow_mut() = None,
            _ => {}
        }
    });

    let field = input.clone();
    input.focus_lost().connect(move |enter: bool, _: Option<InputObject>| {
        let text = field.text();
        if enter && !text.is_empty() {
            ask.fire_server(&[text.into_value()]);
            field.set_text("");
        }
    });
}

