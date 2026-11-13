use client::ZED_URL_SCHEME;
use gpui::{AsyncApp, actions};

actions!(
    cli,
    [
        /// Registers the ade:// URL scheme handler.
        RegisterAdeScheme
    ]
);

pub async fn register_ade_scheme(cx: &AsyncApp) -> anyhow::Result<()> {
    cx.update(|cx| cx.register_url_scheme(ZED_URL_SCHEME)).await
}
