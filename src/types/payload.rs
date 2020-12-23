//! Payload/Bytes/String extractors
use std::future::{ready, Future, Ready};
use std::pin::Pin;
use std::str;
use std::task::{Context, Poll};

use actix_http::error::{Error, ErrorBadRequest, PayloadError};
use actix_http::HttpMessage;
use bytes::{Bytes, BytesMut};
use encoding_rs::UTF_8;
use futures_core::ready;
use futures_core::stream::Stream;
use mime::Mime;

use crate::extract::FromRequest;
use crate::http::header;
use crate::request::HttpRequest;
use crate::{dev, web};

/// Payload extractor returns request 's payload stream.
///
/// ## Example
///
/// ```rust
/// use actix_web::{web, error, App, Error, HttpResponse};
/// use std::future::Future;
/// use futures_core::stream::Stream;
/// use futures_util::StreamExt;
/// /// extract binary data from request
/// async fn index(mut body: web::Payload) -> Result<HttpResponse, Error>
/// {
///     let mut bytes = web::BytesMut::new();
///     while let Some(item) = body.next().await {
///         bytes.extend_from_slice(&item?);
///     }
///
///     format!("Body {:?}!", bytes);
///     Ok(HttpResponse::Ok().finish())
/// }
///
/// fn main() {
///     let app = App::new().service(
///         web::resource("/index.html").route(
///             web::get().to(index))
///     );
/// }
/// ```
pub struct Payload(pub crate::dev::Payload);

impl Payload {
    /// Deconstruct to a inner value
    pub fn into_inner(self) -> crate::dev::Payload {
        self.0
    }
}

impl Stream for Payload {
    type Item = Result<Bytes, PayloadError>;

    #[inline]
    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.0).poll_next(cx)
    }
}

/// Get request's payload stream
///
/// ## Example
///
/// ```rust
/// use actix_web::{web, error, App, Error, HttpResponse};
/// use std::future::Future;
/// use futures_core::stream::Stream;
/// use futures_util::StreamExt;
///
/// /// extract binary data from request
/// async fn index(mut body: web::Payload) -> Result<HttpResponse, Error>
/// {
///     let mut bytes = web::BytesMut::new();
///     while let Some(item) = body.next().await {
///         bytes.extend_from_slice(&item?);
///     }
///
///     format!("Body {:?}!", bytes);
///     Ok(HttpResponse::Ok().finish())
/// }
///
/// fn main() {
///     let app = App::new().service(
///         web::resource("/index.html").route(
///             web::get().to(index))
///     );
/// }
/// ```
impl FromRequest for Payload {
    type Error = Error;
    type Future<'f> = Ready<Result<Payload, Error>>;
    type Config = PayloadConfig;

    #[inline]
    fn from_request<'a>(
        _: &'a HttpRequest,
        payload: &'a mut dev::Payload,
    ) -> Self::Future<'a> {
        ready(Ok(Payload(payload.take())))
    }
}

/// Request binary data from a request's payload.
///
/// Loads request's payload and construct Bytes instance.
///
/// [**PayloadConfig**](PayloadConfig) allows to configure
/// extraction process.
///
/// ## Example
///
/// ```rust
/// use bytes::Bytes;
/// use actix_web::{web, App};
///
/// /// extract binary data from request
/// async fn index(body: Bytes) -> String {
///     format!("Body {:?}!", body)
/// }
///
/// fn main() {
///     let app = App::new().service(
///         web::resource("/index.html").route(
///             web::get().to(index))
///     );
/// }
/// ```
impl FromRequest for Bytes {
    type Error = Error;
    type Future<'f> = impl Future<Output = Result<Self, Error>>;
    type Config = PayloadConfig;

    #[inline]
    fn from_request<'a>(
        req: &'a HttpRequest,
        payload: &'a mut dev::Payload,
    ) -> Self::Future<'a> {
        async move {
            // allow both Config and Data<Config>
            let cfg = PayloadConfig::from_req(req);
            cfg.check_mimetype(req)?;
            let limit = cfg.limit;
            let res = HttpMessageBody::new(req, payload).limit(limit).await?;
            Ok(res)
        }
    }
}

/// Extract text information from a request's body.
///
/// Text extractor automatically decode body according to the request's charset.
///
/// [**PayloadConfig**](PayloadConfig) allows to configure
/// extraction process.
///
/// ## Example
///
/// ```rust
/// use actix_web::{web, App, FromRequest};
///
/// /// extract text data from request
/// async fn index(text: String) -> String {
///     format!("Body {}!", text)
/// }
///
/// fn main() {
///     let app = App::new().service(
///         web::resource("/index.html")
///             .app_data(String::configure(|cfg| {  // <- limit size of the payload
///                 cfg.limit(4096)
///             }))
///             .route(web::get().to(index))  // <- register handler with extractor params
///     );
/// }
/// ```
impl FromRequest for String {
    type Error = Error;
    type Future<'f> = impl Future<Output = Result<Self, Error>>;
    type Config = PayloadConfig;

    #[inline]
    fn from_request<'a>(
        req: &'a HttpRequest,
        payload: &'a mut dev::Payload,
    ) -> Self::Future<'a> {
        async move {
            let cfg = PayloadConfig::from_req(req);

            // check content-type
            cfg.check_mimetype(req)?;

            // check charset
            let encoding = req.encoding()?;

            let limit = cfg.limit;
            let body = HttpMessageBody::new(req, payload).limit(limit).await?;

            if encoding == UTF_8 {
                Ok(str::from_utf8(body.as_ref())
                    .map_err(|_| ErrorBadRequest("Can not decode body"))?
                    .to_owned())
            } else {
                Ok(encoding
                    .decode_without_bom_handling_and_without_replacement(&body)
                    .map(|s| s.into_owned())
                    .ok_or_else(|| ErrorBadRequest("Can not decode body"))?)
            }
        }
    }
}

/// Configuration for request's payload.
///
/// Applies to the built-in `Bytes` and `String` extractors. Note that the Payload extractor does
/// not automatically check conformance with this configuration to allow more flexibility when
/// building extractors on top of `Payload`.
#[derive(Clone)]
pub struct PayloadConfig {
    limit: usize,
    mimetype: Option<Mime>,
}

impl PayloadConfig {
    /// Create `PayloadConfig` instance and set max size of payload.
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            ..Default::default()
        }
    }

    /// Change max size of payload. By default max size is 256Kb
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// Set required mime-type of the request. By default mime type is not
    /// enforced.
    pub fn mimetype(mut self, mt: Mime) -> Self {
        self.mimetype = Some(mt);
        self
    }

    fn check_mimetype(&self, req: &HttpRequest) -> Result<(), Error> {
        // check content-type
        if let Some(ref mt) = self.mimetype {
            match req.mime_type() {
                Ok(Some(ref req_mt)) => {
                    if mt != req_mt {
                        return Err(ErrorBadRequest("Unexpected Content-Type"));
                    }
                }
                Ok(None) => {
                    return Err(ErrorBadRequest("Content-Type is expected"));
                }
                Err(err) => {
                    return Err(err.into());
                }
            }
        }
        Ok(())
    }

    /// Extract payload config from app data. Check both `T` and `Data<T>`, in that order, and fall
    /// back to the default payload config.
    fn from_req(req: &HttpRequest) -> &Self {
        req.app_data::<Self>()
            .or_else(|| req.app_data::<web::Data<Self>>().map(|d| d.as_ref()))
            .unwrap_or(&DEFAULT_CONFIG)
    }
}

// Allow shared refs to default.
const DEFAULT_CONFIG: PayloadConfig = PayloadConfig {
    limit: DEFAULT_CONFIG_LIMIT,
    mimetype: None,
};

const DEFAULT_CONFIG_LIMIT: usize = 262_144; // 2^18 bytes (~256kB)

impl Default for PayloadConfig {
    fn default() -> Self {
        DEFAULT_CONFIG.clone()
    }
}

/// Future that resolves to a complete http message body.
///
/// Load http message body.
///
/// By default only 256Kb payload reads to a memory, then
/// `PayloadError::Overflow` get returned. Use `MessageBody::limit()`
/// method to change upper limit.
pub struct HttpMessageBody {
    limit: usize,
    length: Option<usize>,
    #[cfg(feature = "compress")]
    stream: dev::Decompress<dev::Payload>,
    #[cfg(not(feature = "compress"))]
    stream: dev::Payload,
    buf: BytesMut,
    err: Option<PayloadError>,
}

impl HttpMessageBody {
    /// Create `MessageBody` for request.
    #[allow(clippy::borrow_interior_mutable_const)]
    pub fn new(req: &HttpRequest, payload: &mut dev::Payload) -> HttpMessageBody {
        let mut length = None;
        let mut err = None;

        if let Some(l) = req.headers().get(&header::CONTENT_LENGTH) {
            match l.to_str() {
                Ok(s) => match s.parse::<usize>() {
                    Ok(l) if l > DEFAULT_CONFIG_LIMIT => {
                        err = Some(PayloadError::Overflow)
                    }
                    Ok(l) => length = Some(l),
                    Err(_) => err = Some(PayloadError::UnknownLength),
                },
                Err(_) => err = Some(PayloadError::UnknownLength),
            }
        }

        #[cfg(feature = "compress")]
        let stream = dev::Decompress::from_headers(payload.take(), req.headers());
        #[cfg(not(feature = "compress"))]
        let stream = payload.take();

        HttpMessageBody {
            stream,
            limit: DEFAULT_CONFIG_LIMIT,
            length,
            buf: BytesMut::with_capacity(8192),
            err,
        }
    }

    /// Change max size of payload. By default max size is 256Kb
    pub fn limit(mut self, limit: usize) -> Self {
        if let Some(l) = self.length {
            if l > limit {
                self.err = Some(PayloadError::Overflow);
            }
        }
        self.limit = limit;
        self
    }
}

impl Future for HttpMessageBody {
    type Output = Result<Bytes, PayloadError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        if let Some(e) = this.err.take() {
            return Poll::Ready(Err(e));
        }

        loop {
            let res = ready!(Pin::new(&mut this.stream).poll_next(cx));
            match res {
                Some(chunk) => {
                    let chunk = chunk?;
                    if this.buf.len() + chunk.len() > this.limit {
                        return Poll::Ready(Err(PayloadError::Overflow));
                    } else {
                        this.buf.extend_from_slice(&chunk);
                    }
                }
                None => return Poll::Ready(Ok(this.buf.split().freeze())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::*;
    use crate::http::{header, StatusCode};
    use crate::test::{call_service, init_service, TestRequest};
    use crate::{web, App, Responder};

    #[actix_rt::test]
    async fn test_payload_config() {
        let req = TestRequest::default().to_http_request();
        let cfg = PayloadConfig::default().mimetype(mime::APPLICATION_JSON);
        assert!(cfg.check_mimetype(&req).is_err());

        let req = TestRequest::with_header(
            header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .to_http_request();
        assert!(cfg.check_mimetype(&req).is_err());

        let req = TestRequest::with_header(header::CONTENT_TYPE, "application/json")
            .to_http_request();
        assert!(cfg.check_mimetype(&req).is_ok());
    }

    #[actix_rt::test]
    async fn test_config_recall_locations() {
        async fn bytes_handler(_: Bytes) -> impl Responder {
            "payload is probably json bytes"
        }

        async fn string_handler(_: String) -> impl Responder {
            "payload is probably json string"
        }

        let mut srv = init_service(
            App::new()
                .service(
                    web::resource("/bytes-app-data")
                        .app_data(
                            PayloadConfig::default().mimetype(mime::APPLICATION_JSON),
                        )
                        .route(web::get().to(bytes_handler)),
                )
                .service(
                    web::resource("/bytes-data")
                        .data(PayloadConfig::default().mimetype(mime::APPLICATION_JSON))
                        .route(web::get().to(bytes_handler)),
                )
                .service(
                    web::resource("/string-app-data")
                        .app_data(
                            PayloadConfig::default().mimetype(mime::APPLICATION_JSON),
                        )
                        .route(web::get().to(string_handler)),
                )
                .service(
                    web::resource("/string-data")
                        .data(PayloadConfig::default().mimetype(mime::APPLICATION_JSON))
                        .route(web::get().to(string_handler)),
                ),
        )
        .await;

        let req = TestRequest::with_uri("/bytes-app-data").to_request();
        let resp = call_service(&mut srv, req).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let req = TestRequest::with_uri("/bytes-data").to_request();
        let resp = call_service(&mut srv, req).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let req = TestRequest::with_uri("/string-app-data").to_request();
        let resp = call_service(&mut srv, req).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let req = TestRequest::with_uri("/string-data").to_request();
        let resp = call_service(&mut srv, req).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let req = TestRequest::with_uri("/bytes-app-data")
            .header(header::CONTENT_TYPE, mime::APPLICATION_JSON)
            .to_request();
        let resp = call_service(&mut srv, req).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let req = TestRequest::with_uri("/bytes-data")
            .header(header::CONTENT_TYPE, mime::APPLICATION_JSON)
            .to_request();
        let resp = call_service(&mut srv, req).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let req = TestRequest::with_uri("/string-app-data")
            .header(header::CONTENT_TYPE, mime::APPLICATION_JSON)
            .to_request();
        let resp = call_service(&mut srv, req).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let req = TestRequest::with_uri("/string-data")
            .header(header::CONTENT_TYPE, mime::APPLICATION_JSON)
            .to_request();
        let resp = call_service(&mut srv, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[actix_rt::test]
    async fn test_bytes() {
        let (req, mut pl) = TestRequest::with_header(header::CONTENT_LENGTH, "11")
            .set_payload(Bytes::from_static(b"hello=world"))
            .to_http_parts();

        let s = Bytes::from_request(&req, &mut pl).await.unwrap();
        assert_eq!(s, Bytes::from_static(b"hello=world"));
    }

    #[actix_rt::test]
    async fn test_string() {
        let (req, mut pl) = TestRequest::with_header(header::CONTENT_LENGTH, "11")
            .set_payload(Bytes::from_static(b"hello=world"))
            .to_http_parts();

        let s = String::from_request(&req, &mut pl).await.unwrap();
        assert_eq!(s, "hello=world");
    }

    #[actix_rt::test]
    async fn test_message_body() {
        let (req, mut pl) = TestRequest::with_header(header::CONTENT_LENGTH, "xxxx")
            .to_srv_request()
            .into_parts();
        let res = HttpMessageBody::new(&req, &mut pl).await;
        match res.err().unwrap() {
            PayloadError::UnknownLength => (),
            _ => unreachable!("error"),
        }

        let (req, mut pl) = TestRequest::with_header(header::CONTENT_LENGTH, "1000000")
            .to_srv_request()
            .into_parts();
        let res = HttpMessageBody::new(&req, &mut pl).await;
        match res.err().unwrap() {
            PayloadError::Overflow => (),
            _ => unreachable!("error"),
        }

        let (req, mut pl) = TestRequest::default()
            .set_payload(Bytes::from_static(b"test"))
            .to_http_parts();
        let res = HttpMessageBody::new(&req, &mut pl).await;
        assert_eq!(res.ok().unwrap(), Bytes::from_static(b"test"));

        let (req, mut pl) = TestRequest::default()
            .set_payload(Bytes::from_static(b"11111111111111"))
            .to_http_parts();
        let res = HttpMessageBody::new(&req, &mut pl).limit(5).await;
        match res.err().unwrap() {
            PayloadError::Overflow => (),
            _ => unreachable!("error"),
        }
    }
}
