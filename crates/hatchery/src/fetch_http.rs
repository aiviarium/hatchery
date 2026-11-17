//! A [`Fetch`](crate::Fetch) over HTTP(S) with ureq.

use bytes::Bytes;

use crate::error::Result;
use crate::fetch::{BoxFuture, Fetch, Token, Url};

/// Fetches with a blocking ureq agent. The request runs on the calling
/// thread when the future is polled.
pub struct HttpFetch {
    _private: (),
}

impl HttpFetch {
    pub fn new() -> HttpFetch {
        todo!()
    }
}

impl Default for HttpFetch {
    fn default() -> Self {
        HttpFetch::new()
    }
}

impl Fetch for HttpFetch {
    fn get<'a>(
        &'a self,
        url: &'a Url,
        auth: Option<&'a Token>,
    ) -> BoxFuture<'a, Result<Option<Bytes>>> {
        let _ = (url, auth);
        todo!()
    }
}
