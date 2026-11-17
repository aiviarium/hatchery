//! How bytes are fetched over HTTP. The tool passes its own client; the
//! `http` feature adds one built on ureq.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use bytes::Bytes;
pub use url::Url;

use crate::error::Result;

/// A future the traits of this crate return, so that they can be used as
/// trait objects.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A bearer token, from the environment variable a registry's `token-env`
/// names. Its `Debug` does not show it.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    /// A token with this text. [`Token::from_env`] keeps the secret out of the code.
    pub fn new(secret: impl Into<String>) -> Token {
        Token(secret.into())
    }

    /// The token's text, to put in an `Authorization: Bearer` header.
    pub fn secret(&self) -> &str {
        &self.0
    }

    /// Reads the token from an environment variable. None when unset or empty.
    pub fn from_env(var: &str) -> Option<Token> {
        std::env::var(var).ok().filter(|s| !s.is_empty()).map(Token)
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(…)")
    }
}

/// Fetches a URL with GET.
pub trait Fetch: Send + Sync {
    /// The body of a successful response. A 404 is `Ok(None)`; any other
    /// failure is `Error::Http`.
    fn get<'a>(
        &'a self,
        url: &'a Url,
        auth: Option<&'a Token>,
    ) -> BoxFuture<'a, Result<Option<Bytes>>>;
}

#[cfg(feature = "http")]
pub use crate::fetch_http::HttpFetch;
