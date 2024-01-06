//! Internal implementation details

pub(crate) mod api;
pub(crate) mod content_type;
pub(crate) mod env;
pub(crate) mod http_proxy;
pub(crate) mod hyper_proxy;
pub(crate) mod serde;
pub(crate) mod url;
pub(crate) mod url_parser;

// Internal to imp crate
mod hyper_shims;
mod other;
