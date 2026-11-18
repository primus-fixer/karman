//! Запросы к облаку — общие, из kit (`kit_cloud::http`); здесь — то, что нужно только входу по коду.

pub(crate) use kit_cloud::http::{auth_expired, enc, fail, no_space, odd, server, Body, Resp};
use kit_core::cloud::CloudFail;
use kit_core::Error;

pub(crate) use kit_cloud::http::Http;

/// Запросы от имени «Кармана».
pub(crate) fn http() -> Http {
    Http::new("Karman")
}

pub(crate) fn no_access() -> Error {
    fail(CloudFail::Auth, "Нет доступа к папке «Карман» в облаке — войдите снова")
}
