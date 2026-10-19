//! Existing streamed body checks, used before startup rather than by a deferred loader.
use crate::font_download::FontBody;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

fn error(e: JsValue) -> String {
    e.as_string().unwrap_or_else(|| "the request failed or timed out".into())
}

pub async fn read_response(response: web_sys::Response, asset: lightcraft_engine::fonts::FontAsset) -> Result<Vec<u8>, String> {
    if !response.ok() {
        return Err(format!("the font server returned HTTP {}", response.status()));
    }
    let stream = response.body().ok_or("the font response has no body")?;
    let reader = stream.get_reader().dyn_into::<web_sys::ReadableStreamDefaultReader>().map_err(|e| error(e.into()))?;
    let result = read_body(&reader, asset).await;
    if result.is_err() {
        let _ = JsFuture::from(reader.cancel()).await;
    }
    reader.release_lock();
    result
}

async fn read_body(reader: &web_sys::ReadableStreamDefaultReader, asset: lightcraft_engine::fonts::FontAsset) -> Result<Vec<u8>, String> {
    let mut body = FontBody::new(asset);
    loop {
        let part = JsFuture::from(reader.read()).await.map_err(error)?;
        let done = js_sys::Reflect::get(&part, &"done".into()).map_err(error)?.as_bool().ok_or("invalid font stream")?;
        if done {
            break;
        }
        let chunk = js_sys::Reflect::get(&part, &"value".into()).map_err(error)?.dyn_into::<js_sys::Uint8Array>().map_err(error)?;
        // Check before copying a browser-provided chunk into WASM memory.
        body.append(chunk.length() as usize, |bytes| chunk.copy_to(bytes))?;
    }
    body.finish()
}
