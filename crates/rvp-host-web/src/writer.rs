//! Replacing a library file for the tag editor, through three small functions the page defines (`window.rvpCanWrite`,
//! `window.rvpWriteFile` and `window.rvpWritePoll`, `web/main.js`): the page does the writing with the File System Access API, so the
//! browser's own rules apply (a permission prompt, a swap file that replaces the file only when it is complete). A browser without it,
//! or a folder that was added without a handle, answers with the reason and the editor opens read-only.
use rvp_host::FileWriter;
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    /// `""` when files of the folder can be written (and asks the browser for permission, inside the user's action), else why not.
    #[wasm_bindgen(js_namespace = window, js_name = rvpCanWrite)]
    fn js_can_write(root: &str) -> String;
    /// Start replacing a file; returns a ticket.
    #[wasm_bindgen(js_namespace = window, js_name = rvpWriteFile)]
    fn js_write(root: &str, path: &str, data: &[u8]) -> u32;
    /// `undefined` while pending, else `{ ok, text }`.
    #[wasm_bindgen(js_namespace = window, js_name = rvpWritePoll)]
    fn js_poll(id: u32) -> JsValue;
}

/// The page's [`FileWriter`].
#[derive(Default)]
pub struct WebWriter;

impl FileWriter for WebWriter {
    fn can_write(&mut self, root: &str) -> Result<(), String> {
        let why = js_can_write(root);
        if why.is_empty() { Ok(()) } else { Err(why) }
    }

    fn write(&mut self, root: &str, path: &str, data: Vec<u8>) -> u32 {
        js_write(root, path, &data)
    }

    fn poll_write(&mut self, ticket: u32) -> Option<Result<(), String>> {
        let v = js_poll(ticket);
        if v.is_undefined() || v.is_null() {
            return None;
        }
        let get = |k: &str| js_sys::Reflect::get(&v, &JsValue::from_str(k)).ok();
        let ok = get("ok").and_then(|x| x.as_bool()).unwrap_or(false);
        let text = get("text").and_then(|x| x.as_string()).unwrap_or_default();
        Some(if ok { Ok(()) } else { Err(text) })
    }
}
