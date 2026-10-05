//! A [`Source`] over a browser `File`/`Blob`. Reading goes through `Blob.slice().arrayBuffer()`, which is
//! asynchronous and slow per call, so the source reads 1 MiB chunks, keeps a few, and prefetches the next one.
use rvp_host::{HostError, Source};
use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{JsFuture, spawn_local};

const CHUNK: u64 = 1 << 20;
const KEEP: usize = 4;

type Slot = Rc<RefCell<Option<Result<Rc<Vec<u8>>, String>>>>;

struct Wait(Slot);

impl Future for Wait {
    type Output = Result<Rc<Vec<u8>>, String>;
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        match self.0.borrow_mut().take() {
            Some(v) => Poll::Ready(v),
            None => Poll::Pending,
        }
    }
}

/// A file in the browser.
pub struct WebSource {
    file: web_sys::File,
    name: String,
    size: u64,
    chunks: Vec<(u64, Rc<Vec<u8>>)>,
    inflight: Option<(u64, Slot)>,
}

impl WebSource {
    /// Wrap a `File`.
    pub fn new(file: web_sys::File) -> Self {
        let blob: &web_sys::Blob = file.as_ref();
        let size = blob.size() as u64;
        Self { name: file.name(), file, size, chunks: Vec::new(), inflight: None }
    }

    fn start(&self, idx: u64) -> Slot {
        let slot: Slot = Rc::new(RefCell::new(None));
        let (start, end) = (idx * CHUNK, ((idx + 1) * CHUNK).min(self.size));
        let blob: &web_sys::Blob = self.file.as_ref();
        let out = slot.clone();
        match blob.slice_with_f64_and_f64(start as f64, end as f64) {
            Ok(part) => {
                let promise = part.array_buffer();
                spawn_local(async move {
                    let r = JsFuture::from(promise)
                        .await
                        .map(|buf| Rc::new(js_sys::Uint8Array::new(&buf).to_vec()))
                        .map_err(|e| format!("{e:?}"));
                    *out.borrow_mut() = Some(r);
                });
            }
            Err(e) => *slot.borrow_mut() = Some(Err(format!("{e:?}"))),
        }
        slot
    }

    fn cached(&self, idx: u64) -> Option<Rc<Vec<u8>>> {
        self.chunks.iter().find(|(i, _)| *i == idx).map(|(_, c)| c.clone())
    }

    fn remember(&mut self, idx: u64, data: Rc<Vec<u8>>) {
        self.chunks.retain(|(i, _)| *i != idx);
        self.chunks.push((idx, data));
        if self.chunks.len() > KEEP {
            self.chunks.remove(0);
        }
    }

    async fn chunk(&mut self, idx: u64) -> Result<Rc<Vec<u8>>, HostError> {
        if let Some(c) = self.cached(idx) {
            return Ok(c);
        }
        let slot = match self.inflight.take() {
            Some((i, s)) if i == idx => s,
            other => {
                // A stale prefetch (after a seek) is simply dropped; its result is discarded.
                drop(other);
                self.start(idx)
            }
        };
        let data = Wait(slot).await.map_err(HostError)?;
        self.remember(idx, data.clone());
        Ok(data)
    }
}

impl Source for WebSource {
    async fn size(&self) -> Option<u64> {
        Some(self.size)
    }

    async fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, HostError> {
        if offset >= self.size || buf.is_empty() {
            return Ok(0);
        }
        let idx = offset / CHUNK;
        let chunk = self.chunk(idx).await?;
        // Sequential readers get the next chunk while they work on this one.
        let next = idx + 1;
        if next * CHUNK < self.size && self.inflight.is_none() && self.cached(next).is_none() {
            self.inflight = Some((next, self.start(next)));
        }
        let at = (offset - idx * CHUNK) as usize;
        let n = buf.len().min(chunk.len().saturating_sub(at));
        buf[..n].copy_from_slice(&chunk[at..at + n]);
        Ok(n)
    }

    fn name(&self) -> &str {
        &self.name
    }
}

#[allow(dead_code)]
fn _assert_file_is_blob(f: &web_sys::File) -> &web_sys::Blob {
    f.unchecked_ref()
}
