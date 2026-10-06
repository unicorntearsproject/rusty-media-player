//! Fetching a link for the theme dialog, through two small functions the page defines (`window.rvpFetchText` and `window.rvpFetchPoll`,
//! `web/main.js`): the page does the `fetch`, so the browser's own rules apply. A cross-origin page that does not allow the read fails
//! with the browser's error, and the dialog tells the person to paste the CSS instead.
use rvp_host::Net;
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    /// Start fetching `url`; returns a ticket.
    #[wasm_bindgen(js_namespace = window, js_name = rvpFetchText)]
    fn js_fetch(url: &str, max: u32) -> u32;
    /// `undefined` while pending, else `{ ok, text }`.
    #[wasm_bindgen(js_namespace = window, js_name = rvpFetchPoll)]
    fn js_poll(id: u32) -> JsValue;
}

/// The page's [`Net`].
#[derive(Default)]
pub struct WebNet;

impl Net for WebNet {
    fn fetch_text(&mut self, url: &str) -> u32 {
        js_fetch(url, rvp_host::MAX_FETCH_BYTES as u32)
    }

    fn poll_fetch(&mut self, id: u32) -> Option<Result<String, String>> {
        let v = js_poll(id);
        if v.is_undefined() || v.is_null() {
            return None;
        }
        let get = |k: &str| js_sys::Reflect::get(&v, &JsValue::from_str(k)).ok();
        let ok = get("ok").and_then(|x| x.as_bool()).unwrap_or(false);
        let text = get("text").and_then(|x| x.as_string()).unwrap_or_default();
        Some(if ok { Ok(text) } else { Err(text) })
    }
}
