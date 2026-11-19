//! The host messages between a Craft app's web build and the page that hosts it: one contract for
//! the whole family (storytold/craft-libs#358), so each app's bridge is a few calls instead of its
//! own copy.
//!
//! Messages are plain objects with a `type` of `<app>:<kind>` (`cadcraft:open`):
//!
//! | Kind | Direction | Fields |
//! |---|---|---|
//! | `ready` | app → host | `version`, `appVersion?`, `file?` (`true`: it reads a `File`/`Blob` in slices) |
//! | `host` | host → app | `version`, `handoff?` (`true`: the host routes hand-offs) |
//! | `open` | host → app | `name`, and `bytes` or `file` + `size` |
//! | `attach-pdf` | host → app | `name`, `bytes`, `page` (from 1), `insertion` ([x,y,z]), `width`, `height` in drawing units |
//! | `fonts` | host → app | `indexes`: up to [`MAX_FONT_INDEXES`] entries, each `{bytes}` or `{url, sha256}` (see [`FontIndex`]) |
//! | `save` | app → host | `name`, `bytes`, `id?`, `saveAs?` |
//! | `saved` | host → app | `id` |
//! | `dirty` | app → host | `dirty` |
//! | `failed` | app → host | `error`, `id?` |
//! | `handoff` | app → host | `name`, and `bytes` or `file` + `size` |
//! | `handoff-done` | host → app | `name`, `app` |
//!
//! `fonts` hands the app font indexes in craft-font-index format (RFC-0034, storytold/craft-libs#2331).
//! Only the host sends it: a document, an agent or a script cannot, and the host's sending it is
//! its consent to fetch from those indexes. The bridge only checks the shape of the message
//! ([`FontIndex`]); it does not parse an index, so the app checks each one itself
//! (`craft-font-store`'s `Index::from_user_bytes` rules) and takes the whole list or none of it.
//! The bridge drops a list it refuses whole, as it drops any malformed message.
//! Each `fonts` message gives the host's whole set and replaces the set it gave before: a host adds
//! an index by sending its set again with it, and an index it leaves out is gone. When messages
//! come faster than an app reads their indexes, the last one wins.
//!
//! A host answers `saved` only for a `save` that carries an `id`; an app that sends none treats a
//! save as done once it is posted. Fields marked `?` are optional, so the bridges shipped before
//! this crate already speak version 1. Those bridges put the app's own release in `ready`'s
//! `version` (`"0.14.0"`): a host treats any `version` it doesn't know as `"1"`.
//!
//! The app is hosted when its URL has `?host=parent` (framed by a same-origin page: messages go
//! through `postMessage`) or its canvas has `data-host="page"` (the page runs it directly:
//! messages are `CustomEvent`s on the canvas, the fields in `detail`). This module is the contract
//! itself and builds everywhere; [`web`] (wasm32 only) is the bridge an app's web shell calls.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

#[cfg(target_arch = "wasm32")]
pub mod web;

/// The contract version an app reports in `ready` and a host in `host`. It changes only when a
/// field changes meaning; new optional fields keep it.
pub const VERSION: &str = "1";

/// Longest file name taken from a host or put in a message, in characters.
pub const MAX_NAME_CHARS: usize = 255;

/// The most indexes one `fonts` message may carry.
pub const MAX_FONT_INDEXES: usize = 16;

/// The most bytes one index's `bytes` may hold (the size `craft-font-store` reads an index up to).
pub const MAX_FONT_INDEX_BYTES: usize = 32 << 20;

/// The longest index URL taken, in bytes.
pub const MAX_FONT_INDEX_URL_BYTES: usize = 2048;

/// A font index a host gives, checked for shape only: the app parses it and refuses a bad one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FontIndex {
    /// An index in craft-font-index format (JSON or gzipped JSON), at most [`MAX_FONT_INDEX_BYTES`].
    Bytes(Vec<u8>),
    /// An index to fetch: an `https` URL and the lowercase hex sha256 of the file there.
    Url { url: String, sha256: String },
}

/// Why a `fonts` message is refused. The whole message is: nothing is taken from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontsProblem {
    /// `indexes` is missing or not an array.
    NotAList,
    /// More than [`MAX_FONT_INDEXES`] entries.
    TooMany,
    /// An entry that is not an object with exactly one of `bytes` and `url` (`sha256` goes with `url`).
    BadEntry,
    /// `bytes` over [`MAX_FONT_INDEX_BYTES`].
    TooLarge,
    /// A `url` that is not `https://` with a host, or is too long or holds spaces or control characters.
    BadUrl,
    /// A `sha256` that is not 64 lowercase hex digits.
    BadSha256,
}

impl FontIndex {
    /// An index by its bytes; `len` is checked before the bytes are copied (see [`check_font_index_len`]).
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, FontsProblem> {
        check_font_index_len(bytes.len())?;
        Ok(Self::Bytes(bytes))
    }

    /// An index by its `https` URL and the sha256 of the file there.
    pub fn from_url(url: &str, sha256: &str) -> Result<Self, FontsProblem> {
        let rest = url.strip_prefix("https://").ok_or(FontsProblem::BadUrl)?;
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if url.len() > MAX_FONT_INDEX_URL_BYTES || host.is_empty() || host.contains('@') || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(FontsProblem::BadUrl);
        }
        if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err(FontsProblem::BadSha256);
        }
        Ok(Self::Url { url: url.to_string(), sha256: sha256.to_string() })
    }
}

/// Refuse an index of `len` bytes over [`MAX_FONT_INDEX_BYTES`], before it is copied.
pub fn check_font_index_len(len: usize) -> Result<(), FontsProblem> {
    if len > MAX_FONT_INDEX_BYTES { Err(FontsProblem::TooLarge) } else { Ok(()) }
}

/// Refuse a list of `count` entries over [`MAX_FONT_INDEXES`], before any is read.
pub fn check_font_index_count(count: usize) -> Result<(), FontsProblem> {
    if count > MAX_FONT_INDEXES { Err(FontsProblem::TooMany) } else { Ok(()) }
}

/// How a page hosts the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `?host=parent`: a same-origin page frames the app; messages go through `postMessage`.
    Parent,
    /// `data-host="page"` on the app's canvas: the page runs the app itself; messages are
    /// `CustomEvent`s on the canvas.
    Page,
}

impl Mode {
    /// The mode for a page URL's query (with or without its leading `?`) and the canvas's
    /// `data-host` attribute. `?host=parent` wins; anything else is not hosted.
    pub fn detect(query: &str, data_host: Option<&str>) -> Option<Self> {
        let query = query.strip_prefix('?').unwrap_or(query);
        if query.split('&').any(|pair| pair == "host=parent") {
            Some(Self::Parent)
        } else if data_host == Some("page") {
            Some(Self::Page)
        } else {
            None
        }
    }
}

/// What an app posts to its host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outgoing {
    Ready,
    Save,
    Dirty,
    Failed,
    Handoff,
}

/// What a host sends to an app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Incoming {
    Host,
    Open,
    AttachPdf,
    Fonts,
    Saved,
    HandoffDone,
}

impl Outgoing {
    pub const fn kind(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Save => "save",
            Self::Dirty => "dirty",
            Self::Failed => "failed",
            Self::Handoff => "handoff",
        }
    }
}

impl Incoming {
    pub const ALL: [Self; 6] = [Self::Host, Self::Open, Self::AttachPdf, Self::Fonts, Self::Saved, Self::HandoffDone];

    pub const fn kind(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Open => "open",
            Self::AttachPdf => "attach-pdf",
            Self::Fonts => "fonts",
            Self::Saved => "saved",
            Self::HandoffDone => "handoff-done",
        }
    }

    /// The incoming kind a message `type` names for `app`, or `None` for another app's message or
    /// one this contract doesn't send to apps.
    pub fn parse(app: &str, message_type: &str) -> Option<Self> {
        let kind = message_type.strip_prefix(app)?.strip_prefix(':')?;
        Self::ALL.into_iter().find(|k| k.kind() == kind)
    }
}

/// The `type` of a message: `<app>:<kind>`.
pub fn message_type(app: &str, kind: &str) -> String {
    format!("{app}:{kind}")
}

/// A file name from a host, made safe to show and save under: path separators and control
/// characters dropped, surrounding space trimmed, at most [`MAX_NAME_CHARS`] characters, and
/// `fallback` when nothing is left.
pub fn file_name(raw: Option<&str>, fallback: &str) -> String {
    let base = raw.unwrap_or_default().rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = base.chars().filter(|c| !c.is_control()).collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return fallback.to_string();
    }
    trimmed.chars().take(MAX_NAME_CHARS).collect()
}

/// Remember `next` before the send. A page host can, while that send is still
/// running, report a different state; that later report must win. A failed send
/// restores the previous state only when nothing else recorded one.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn note_dirty(dirty: &Cell<Option<bool>>, next: bool, post: impl FnOnce() -> bool) {
    if dirty.get() == Some(next) {
        return;
    }
    let previous = dirty.replace(Some(next));
    if !post() && dirty.get() == Some(next) {
        dirty.set(previous);
    }
}

/// Hands the host's messages to the app's handler one at a time. A page host dispatches its events
/// synchronously, so it can answer while the handler runs (a `saved` for the `save` the handler
/// just posted): that message waits here and is handled as soon as the running call returns, never
/// dropped and never given to a second, overlapping call.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) struct Inbox<M, F> {
    handler: RefCell<F>,
    queue: RefCell<VecDeque<M>>,
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
impl<M, F: FnMut(M)> Inbox<M, F> {
    pub(crate) fn new(handler: F) -> Self {
        Self { handler: RefCell::new(handler), queue: RefCell::new(VecDeque::new()) }
    }

    pub(crate) fn deliver(&self, message: M) {
        self.queue.borrow_mut().push_back(message);
        // Already running: the outer call drains the queue.
        let Ok(mut handler) = self.handler.try_borrow_mut() else { return };
        loop {
            let next = self.queue.borrow_mut().pop_front();
            let Some(message) = next else { break };
            handler(message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_query_switch_wins_over_the_canvas_attribute() {
        assert_eq!(Mode::detect("?host=parent", None), Some(Mode::Parent));
        assert_eq!(Mode::detect("file=a.pdf&host=parent", Some("page")), Some(Mode::Parent));
        assert_eq!(Mode::detect("", Some("page")), Some(Mode::Page));
        assert_eq!(Mode::detect("?host=parents", None), None);
        assert_eq!(Mode::detect("?xhost=parent", None), None);
        assert_eq!(Mode::detect("", Some("frame")), None);
    }

    #[test]
    fn only_this_apps_incoming_kinds_parse() {
        assert_eq!(Incoming::parse("pdfcraft", "pdfcraft:open"), Some(Incoming::Open));
        assert_eq!(Incoming::parse("pdfcraft", "pdfcraft:handoff-done"), Some(Incoming::HandoffDone));
        assert_eq!(Incoming::parse("pdfcraft", "cadcraft:open"), None, "another app's message");
        assert_eq!(Incoming::parse("pdfcraft", "pdfcraft:save"), None, "app-to-host kinds are not incoming");
        assert_eq!(Incoming::parse("pdf", "pdfcraft:open"), None, "an app id that is only a prefix");
        assert_eq!(Incoming::parse("pdfcraft", "pdfcraftopen"), None);
    }

    #[test]
    fn every_kind_round_trips_through_its_type() {
        for kind in Incoming::ALL {
            assert_eq!(Incoming::parse("gridcraft", &message_type("gridcraft", kind.kind())), Some(kind));
        }
        assert_eq!(message_type("cadcraft", Outgoing::Handoff.kind()), "cadcraft:handoff");
    }

    #[test]
    fn fonts_is_an_incoming_kind_only() {
        assert_eq!(Incoming::parse("pdfcraft", "pdfcraft:fonts"), Some(Incoming::Fonts));
        assert_eq!(Incoming::Fonts.kind(), "fonts");
    }

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn both_forms_of_a_font_index_are_taken() {
        assert_eq!(FontIndex::from_bytes(vec![1, 2, 3]), Ok(FontIndex::Bytes(vec![1, 2, 3])));
        assert_eq!(FontIndex::from_bytes(vec![0; MAX_FONT_INDEX_BYTES]).map(|_| ()), Ok(()));
        assert_eq!(
            FontIndex::from_url("https://fonts.example/index.json?v=2", SHA),
            Ok(FontIndex::Url { url: "https://fonts.example/index.json?v=2".into(), sha256: SHA.into() })
        );
        assert_eq!(check_font_index_count(MAX_FONT_INDEXES), Ok(()));
    }

    #[test]
    fn a_font_index_over_a_bound_is_refused() {
        assert_eq!(check_font_index_count(MAX_FONT_INDEXES + 1), Err(FontsProblem::TooMany));
        assert_eq!(check_font_index_len(MAX_FONT_INDEX_BYTES), Ok(()));
        assert_eq!(check_font_index_len(MAX_FONT_INDEX_BYTES + 1), Err(FontsProblem::TooLarge));
        assert_eq!(FontIndex::from_bytes(vec![0; MAX_FONT_INDEX_BYTES + 1]), Err(FontsProblem::TooLarge));
        let long = format!("https://a.example/{}", "x".repeat(MAX_FONT_INDEX_URL_BYTES));
        assert_eq!(FontIndex::from_url(&long, SHA), Err(FontsProblem::BadUrl));
    }

    #[test]
    fn only_an_https_url_with_a_host_is_taken() {
        for url in [
            "http://fonts.example/i.json",
            "HTTPS://fonts.example/i.json",
            "ftp://fonts.example/i.json",
            "//fonts.example/i.json",
            "fonts.example/i.json",
            "file:///etc/passwd",
            "data:application/json,{}",
            "javascript:alert(1)",
            "https://",
            "https:///i.json",
            "https://?x=1",
            "https://user:pw@fonts.example/i.json",
            "https://fonts.example/a b.json",
            "https://fonts.example/a\nb.json",
            "https://fonts.example/\u{0}",
            "",
        ] {
            assert_eq!(FontIndex::from_url(url, SHA), Err(FontsProblem::BadUrl), "{url:?}");
        }
    }

    #[test]
    fn a_sha256_is_sixty_four_lowercase_hex_digits() {
        let url = "https://fonts.example/i.json";
        for sha in ["", "abc", &SHA[1..], &format!("{SHA}0"), &SHA.to_uppercase(), &format!("g{}", &SHA[1..]), &format!(" {}", &SHA[1..])] {
            assert_eq!(FontIndex::from_url(url, sha), Err(FontsProblem::BadSha256), "{sha:?}");
        }
    }

    #[test]
    fn host_file_names_are_cleaned() {
        assert_eq!(file_name(Some("plan.dxf"), "Drawing.dxf"), "plan.dxf");
        assert_eq!(file_name(Some("  ../../etc/passwd "), "x"), "passwd");
        assert_eq!(file_name(Some("C:\\Users\\sam\\notes.pdf"), "x"), "notes.pdf");
        assert_eq!(file_name(Some("a\u{0}b\nc.txt"), "x"), "abc.txt");
        assert_eq!(file_name(Some("dir/"), "Drawing.dxf"), "Drawing.dxf");
        assert_eq!(file_name(Some(".."), "x"), "x");
        assert_eq!(file_name(None, "Document.pdf"), "Document.pdf");
        assert_eq!(file_name(Some(&"é".repeat(300)), "x").chars().count(), MAX_NAME_CHARS);
    }

    #[test]
    fn a_dirty_report_during_the_send_wins_and_a_failed_send_is_forgotten() {
        let dirty = Cell::new(Some(false));
        note_dirty(&dirty, true, || {
            note_dirty(&dirty, false, || true);
            true
        });
        assert_eq!(dirty.get(), Some(false));

        let dirty = Cell::new(None);
        note_dirty(&dirty, true, || false);
        assert_eq!(dirty.get(), None);

        let dirty = Cell::new(Some(true));
        let mut sent = false;
        note_dirty(&dirty, true, || {
            sent = true;
            true
        });
        assert!(!sent);
        assert_eq!(dirty.get(), Some(true));
    }

    #[test]
    fn malformed_message_types_name_no_kind() {
        for bad in
            ["", ":", "pdfcraft", "pdfcraft:", ":open", "pdfcraft:open:extra", "pdfcraft:OPEN", "pdfcraft: open", "pdfcraftx:open", "pdfcraft/open"]
        {
            assert_eq!(Incoming::parse("pdfcraft", bad), None, "{bad:?}");
        }
        assert_eq!(Incoming::parse("pdfcraft", "pdfcraft:open"), Some(Incoming::Open));
        // The app's own outgoing kinds are not messages to it.
        assert_eq!(Incoming::parse("pdfcraft", "pdfcraft:save"), None);
    }

    #[test]
    fn host_file_names_with_hostile_parts_are_cleaned() {
        // Only control characters, or only separators: nothing is left, so the fallback.
        assert_eq!(file_name(Some("\u{0}\u{7}\n\t"), "Drawing.dxf"), "Drawing.dxf");
        assert_eq!(file_name(Some("///"), "Drawing.dxf"), "Drawing.dxf");
        assert_eq!(file_name(Some("   "), "Drawing.dxf"), "Drawing.dxf");
        // Mixed separators and a control character inside the last part.
        assert_eq!(file_name(Some("a/b\\c\u{1b}[31m.txt"), "x"), "c[31m.txt");
        // Exactly at the limit stays whole; one over is cut; the limit counts characters, not bytes.
        assert_eq!(file_name(Some(&"a".repeat(MAX_NAME_CHARS)), "x").chars().count(), MAX_NAME_CHARS);
        assert_eq!(file_name(Some(&"a".repeat(MAX_NAME_CHARS + 1)), "x").chars().count(), MAX_NAME_CHARS);
        assert_eq!(file_name(Some(&"\u{1F4C4}".repeat(300)), "x"), "\u{1F4C4}".repeat(MAX_NAME_CHARS));
    }

    #[test]
    fn messages_that_arrive_while_the_handler_runs_wait_their_turn() {
        use std::rc::Rc;
        type Shared = Rc<Inbox<&'static str, Box<dyn FnMut(&'static str)>>>;
        let seen = Rc::new(RefCell::new(Vec::new()));
        let inbox: Rc<RefCell<Option<Shared>>> = Rc::new(RefCell::new(None));
        let (log, me) = (seen.clone(), inbox.clone());
        let handler: Box<dyn FnMut(&'static str)> = Box::new(move |m: &'static str| {
            log.borrow_mut().push(format!("start {m}"));
            // The page answers inside the handler, as a synchronous dispatchEvent does.
            if m == "open"
                && let Some(inbox) = me.borrow().clone()
            {
                inbox.deliver("saved");
                inbox.deliver("handoff-done");
            }
            log.borrow_mut().push(format!("end {m}"));
        });
        let shared = Rc::new(Inbox::new(handler));
        *inbox.borrow_mut() = Some(shared.clone());
        shared.deliver("open");
        assert_eq!(*seen.borrow(), ["start open", "end open", "start saved", "end saved", "start handoff-done", "end handoff-done"]);
        shared.deliver("host");
        assert_eq!(seen.borrow().len(), 8, "the inbox is usable again after draining");
    }
}
