//! A [`Fetch`](crate::Fetch) over HTTP(S) with ureq.

use bytes::Bytes;

use crate::error::{Error, Result};
use crate::fetch::{BoxFuture, Fetch, Token, Url};

/// The most a response body may hold, the size limit of an egg.
const BODY_LIMIT: u64 = 512 * 1024 * 1024;

/// Fetches with a blocking ureq agent. The request runs on the calling
/// thread when the future is polled.
pub struct HttpFetch {
    agent: ureq::Agent,
}

impl HttpFetch {
    pub fn new() -> HttpFetch {
        HttpFetch {
            agent: ureq::Agent::new_with_defaults(),
        }
    }

    fn get_blocking(&self, url: &Url, auth: Option<&Token>) -> Result<Option<Bytes>> {
        let fail = |message: String| Error::Http {
            url: url.to_string(),
            message,
        };
        let mut req = self.agent.get(url.as_str());
        if let Some(token) = auth {
            req = req.header("Authorization", format!("Bearer {}", token.secret()));
        }
        let mut response = match req.call() {
            Ok(r) => r,
            Err(ureq::Error::StatusCode(404)) => return Ok(None),
            Err(e) => return Err(fail(e.to_string())),
        };
        let body = response
            .body_mut()
            .with_config()
            .limit(BODY_LIMIT)
            .read_to_vec()
            .map_err(|e| fail(e.to_string()))?;
        Ok(Some(Bytes::from(body)))
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
        Box::pin(async move { self.get_blocking(url, auth) })
    }
}
