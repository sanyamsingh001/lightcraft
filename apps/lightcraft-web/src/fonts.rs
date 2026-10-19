//! One deferred same-origin font request for the page, never for render workers.
use crate::font_download::FontBody;
use lightcraft_ui_egui::LightcraftApp;
use std::{cell::RefCell, rc::Rc};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::JsFuture;

#[derive(Default)]
pub struct Loader {
    completed: Rc<RefCell<Option<Result<egui::FontData, String>>>>,
    requests: u32,
}
impl Loader {
    pub fn requests(&self) -> u32 {
        self.requests
    }
    pub fn poll(&mut self, app: &mut LightcraftApp, ctx: &egui::Context) {
        if let Some(result) = self.completed.borrow_mut().take() {
            let error = result.as_ref().err().cloned();
            app.runtime_ui_font.finish(result);
            if let Some(error) = error {
                app.toast(ctx, format!("Chinese text could not be loaded: {error}. Choose Chinese again in Edit > Language or Settings to retry."));
            }
        }
    }
    pub fn start(&mut self, app: &mut LightcraftApp, ctx: &egui::Context) {
        if !app.runtime_ui_font.take_request() {
            return;
        }
        self.requests = self.requests.saturating_add(1);
        let completed = self.completed.clone();
        let ctx = ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = match lightcraft_engine::fonts::WEB_CHINESE_FONT {
                Some(asset) => fetch(asset).await.map(egui::FontData::from_owned),
                None => Err("this web build does not include the Chinese font asset".into()),
            };
            *completed.borrow_mut() = Some(result);
            ctx.request_repaint();
        });
    }
}

fn error(e: JsValue) -> String {
    e.as_string().unwrap_or_else(|| "the request failed or timed out".into())
}

async fn fetch(asset: lightcraft_engine::fonts::FontAsset) -> Result<Vec<u8>, String> {
    let window = web_sys::window().ok_or("no browser window")?;
    let base = window.location().href().map_err(error)?;
    let url = web_sys::Url::new_with_base(asset.url, &base).map_err(error)?;
    if url.origin() != window.location().origin().map_err(error)? {
        return Err("the font must be served from this site".into());
    }
    let abort = web_sys::AbortController::new().map_err(error)?;
    let timeout_abort = abort.clone();
    let timeout = Closure::<dyn FnMut()>::new(move || timeout_abort.abort());
    let timer = window.set_timeout_with_callback_and_timeout_and_arguments_0(timeout.as_ref().unchecked_ref(), 30_000).map_err(error)?;
    let init = web_sys::RequestInit::new();
    init.set_mode(web_sys::RequestMode::SameOrigin);
    init.set_credentials(web_sys::RequestCredentials::SameOrigin);
    init.set_redirect(web_sys::RequestRedirect::Error);
    init.set_signal(Some(&abort.signal()));
    let result = fetch_body(&window, &url.href(), &init, asset).await;
    window.clear_timeout_with_handle(timer);
    result
}

async fn fetch_body(
    window: &web_sys::Window,
    url: &str,
    init: &web_sys::RequestInit,
    asset: lightcraft_engine::fonts::FontAsset,
) -> Result<Vec<u8>, String> {
    let response = JsFuture::from(window.fetch_with_str_and_init(url, init)).await.map_err(error)?.dyn_into::<web_sys::Response>().map_err(error)?;
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
