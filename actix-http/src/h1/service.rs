use std::future::{ready, Future, Ready};
use std::marker::PhantomData;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};
use std::{fmt, net};

use actix_codec::Framed;
use actix_rt::net::ServiceStream;
use actix_service::{pipeline_factory, IntoServiceFactory, Service, ServiceFactory};
use futures_core::ready;
use futures_util::StreamExt;

use crate::body::MessageBody;
use crate::cloneable::CloneableService;
use crate::config::ServiceConfig;
use crate::error::{DispatchError, Error, ParseError};
use crate::helpers::DataFactory;
use crate::request::Request;
use crate::response::Response;
use crate::{ConnectCallback, Extensions};

use super::codec::Codec;
use super::dispatcher::Dispatcher;
use super::{ExpectHandler, Message, UpgradeHandler};

/// `ServiceFactory` implementation for HTTP1 transport
pub struct H1Service<T: ServiceStream, S, B, X = ExpectHandler, U = UpgradeHandler<T>> {
    srv: S,
    cfg: ServiceConfig<T::Runtime>,
    expect: X,
    upgrade: Option<U>,
    on_connect: Option<Rc<dyn Fn(&T) -> Box<dyn DataFactory>>>,
    on_connect_ext: Option<Rc<ConnectCallback<T>>>,
    _t: PhantomData<B>,
}

impl<T, S, B> H1Service<T, S, B>
where
    T: ServiceStream,
    S: ServiceFactory<Config = (), Request = Request>,
    S::Error: Into<Error>,
    S::InitError: fmt::Debug,
    S::Response: Into<Response<B>>,
    B: MessageBody,
{
    /// Create new `HttpService` instance with config.
    pub(crate) fn with_config<F: IntoServiceFactory<S>>(
        cfg: ServiceConfig<T::Runtime>,
        service: F,
    ) -> Self {
        H1Service {
            cfg,
            srv: service.into_factory(),
            expect: ExpectHandler,
            upgrade: None,
            on_connect: None,
            on_connect_ext: None,
            _t: PhantomData,
        }
    }
}

impl<T, S, B, X, U> H1Service<T, S, B, X, U>
where
    T: ServiceStream,
    S: ServiceFactory<Config = (), Request = Request>,
    S::Error: Into<Error>,
    S::InitError: fmt::Debug,
    S::Response: Into<Response<B>>,
    B: MessageBody,
    X: ServiceFactory<Config = (), Request = Request, Response = Request>,
    X::Error: Into<Error>,
    X::InitError: fmt::Debug,
    U: ServiceFactory<
        Config = (),
        Request = (Request, Framed<T, Codec<T::Runtime>>),
        Response = (),
    >,
    U::Error: fmt::Display + Into<Error>,
    U::InitError: fmt::Debug,
{
    /// Create simple tcp stream service
    pub fn tcp(
        self,
    ) -> impl ServiceFactory<
        Config = (),
        Request = T,
        Response = (),
        Error = DispatchError,
        InitError = (),
    > {
        pipeline_factory(|io: T| {
            let peer_addr = io.peer_addr();
            ready(Ok((io, peer_addr)))
        })
        .and_then(self)
    }
}

#[cfg(feature = "openssl")]
mod openssl {
    use super::*;

    use actix_tls::openssl::{Acceptor, SslAcceptor, SslStream};
    use actix_tls::{openssl::Error as SslError, TlsError};

    impl<T, S, B, X, U> H1Service<SslStream<T>, S, B, X, U>
    where
        T: ServiceStream,
        S: ServiceFactory<Config = (), Request = Request>,
        S::Error: Into<Error>,
        S::InitError: fmt::Debug,
        S::Response: Into<Response<B>>,
        B: MessageBody,
        X: ServiceFactory<Config = (), Request = Request, Response = Request>,
        X::Error: Into<Error>,
        X::InitError: fmt::Debug,
        U: ServiceFactory<
            Config = (),
            Request = (Request, Framed<SslStream<T>, Codec<T::Runtime>>),
            Response = (),
        >,
        U::Error: fmt::Display + Into<Error>,
        U::InitError: fmt::Debug,
    {
        /// Create openssl based service
        pub fn openssl(
            self,
            acceptor: SslAcceptor,
        ) -> impl ServiceFactory<
            Config = (),
            Request = T,
            Response = (),
            Error = TlsError<SslError, DispatchError>,
            InitError = (),
        > {
            pipeline_factory(
                Acceptor::new(acceptor)
                    .map_err(TlsError::Tls)
                    .map_init_err(|_| panic!()),
            )
            .and_then(|io: SslStream<T>| {
                let peer_addr = io.peer_addr();
                ready(Ok((io, peer_addr)))
            })
            .and_then(self.map_err(TlsError::Service))
        }
    }
}

#[cfg(feature = "rustls")]
mod rustls {
    use super::*;
    use actix_tls::rustls::{Acceptor, ServerConfig, TlsStream};
    use actix_tls::TlsError;
    use std::{fmt, io};

    impl<T, S, B, X, U> H1Service<TlsStream<T>, S, B, X, U>
    where
        T: ServiceStream,
        S: ServiceFactory<Config = (), Request = Request>,
        S::Error: Into<Error>,
        S::InitError: fmt::Debug,
        S::Response: Into<Response<B>>,
        B: MessageBody,
        X: ServiceFactory<Config = (), Request = Request, Response = Request>,
        X::Error: Into<Error>,
        X::InitError: fmt::Debug,
        U: ServiceFactory<
            Config = (),
            Request = (Request, Framed<TlsStream<T>, Codec<T::Runtime>>),
            Response = (),
        >,
        U::Error: fmt::Display + Into<Error>,
        U::InitError: fmt::Debug,
    {
        /// Create rustls based service
        pub fn rustls(
            self,
            config: ServerConfig,
        ) -> impl ServiceFactory<
            Config = (),
            Request = T,
            Response = (),
            Error = TlsError<io::Error, DispatchError>,
            InitError = (),
        > {
            pipeline_factory(
                Acceptor::new(config)
                    .map_err(TlsError::Tls)
                    .map_init_err(|_| panic!()),
            )
            .and_then(|io: TlsStream<T>| {
                let peer_addr = io.peer_addr();
                ready(Ok((io, peer_addr)))
            })
            .and_then(self.map_err(TlsError::Service))
        }
    }
}

impl<T, S, B, X, U> H1Service<T, S, B, X, U>
where
    T: ServiceStream,
    S: ServiceFactory<Config = (), Request = Request>,
    S::Error: Into<Error>,
    S::Response: Into<Response<B>>,
    S::InitError: fmt::Debug,
    B: MessageBody,
{
    pub fn expect<X1>(self, expect: X1) -> H1Service<T, S, B, X1, U>
    where
        X1: ServiceFactory<Request = Request, Response = Request>,
        X1::Error: Into<Error>,
        X1::InitError: fmt::Debug,
    {
        H1Service {
            expect,
            cfg: self.cfg,
            srv: self.srv,
            upgrade: self.upgrade,
            on_connect: self.on_connect,
            on_connect_ext: self.on_connect_ext,
            _t: PhantomData,
        }
    }

    pub fn upgrade<U1>(self, upgrade: Option<U1>) -> H1Service<T, S, B, X, U1>
    where
        U1: ServiceFactory<
            Request = (Request, Framed<T, Codec<T::Runtime>>),
            Response = (),
        >,
        U1::Error: fmt::Display,
        U1::InitError: fmt::Debug,
    {
        H1Service {
            upgrade,
            cfg: self.cfg,
            srv: self.srv,
            expect: self.expect,
            on_connect: self.on_connect,
            on_connect_ext: self.on_connect_ext,
            _t: PhantomData,
        }
    }

    /// Set on connect callback.
    pub(crate) fn on_connect(
        mut self,
        f: Option<Rc<dyn Fn(&T) -> Box<dyn DataFactory>>>,
    ) -> Self {
        self.on_connect = f;
        self
    }

    /// Set on connect callback.
    pub(crate) fn on_connect_ext(mut self, f: Option<Rc<ConnectCallback<T>>>) -> Self {
        self.on_connect_ext = f;
        self
    }
}

impl<T, S, B, X, U> ServiceFactory for H1Service<T, S, B, X, U>
where
    T: ServiceStream,
    S: ServiceFactory<Config = (), Request = Request>,
    S::Error: Into<Error>,
    S::Response: Into<Response<B>>,
    S::InitError: fmt::Debug,
    B: MessageBody,
    X: ServiceFactory<Config = (), Request = Request, Response = Request>,
    X::Error: Into<Error>,
    X::InitError: fmt::Debug,
    U: ServiceFactory<
        Config = (),
        Request = (Request, Framed<T, Codec<T::Runtime>>),
        Response = (),
    >,
    U::Error: fmt::Display + Into<Error>,
    U::InitError: fmt::Debug,
{
    type Request = (T, Option<net::SocketAddr>);
    type Response = ();
    type Error = DispatchError;
    type Config = ();
    type Service = H1ServiceHandler<T, S::Service, B, X::Service, U::Service>;
    type InitError = ();
    type Future = H1ServiceResponse<T, S, B, X, U>;

    fn new_service(&self, _: ()) -> Self::Future {
        H1ServiceResponse {
            fut: self.srv.new_service(()),
            fut_ex: Some(self.expect.new_service(())),
            fut_upg: self.upgrade.as_ref().map(|f| f.new_service(())),
            expect: None,
            upgrade: None,
            on_connect: self.on_connect.clone(),
            on_connect_ext: self.on_connect_ext.clone(),
            cfg: Some(self.cfg.clone()),
            _t: PhantomData,
        }
    }
}

#[doc(hidden)]
#[pin_project::pin_project]
pub struct H1ServiceResponse<T, S, B, X, U>
where
    T: ServiceStream,
    S: ServiceFactory<Request = Request>,
    S::Error: Into<Error>,
    S::InitError: fmt::Debug,
    X: ServiceFactory<Request = Request, Response = Request>,
    X::Error: Into<Error>,
    X::InitError: fmt::Debug,
    U: ServiceFactory<Request = (Request, Framed<T, Codec<T::Runtime>>), Response = ()>,
    U::Error: fmt::Display,
    U::InitError: fmt::Debug,
{
    #[pin]
    fut: S::Future,
    #[pin]
    fut_ex: Option<X::Future>,
    #[pin]
    fut_upg: Option<U::Future>,
    expect: Option<X::Service>,
    upgrade: Option<U::Service>,
    on_connect: Option<Rc<dyn Fn(&T) -> Box<dyn DataFactory>>>,
    on_connect_ext: Option<Rc<ConnectCallback<T>>>,
    cfg: Option<ServiceConfig<T::Runtime>>,
    _t: PhantomData<B>,
}

impl<T, S, B, X, U> Future for H1ServiceResponse<T, S, B, X, U>
where
    T: ServiceStream,
    S: ServiceFactory<Request = Request>,
    S::Error: Into<Error>,
    S::Response: Into<Response<B>>,
    S::InitError: fmt::Debug,
    B: MessageBody,
    X: ServiceFactory<Request = Request, Response = Request>,
    X::Error: Into<Error>,
    X::InitError: fmt::Debug,
    U: ServiceFactory<Request = (Request, Framed<T, Codec<T::Runtime>>), Response = ()>,
    U::Error: fmt::Display,
    U::InitError: fmt::Debug,
{
    type Output = Result<H1ServiceHandler<T, S::Service, B, X::Service, U::Service>, ()>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.as_mut().project();

        if let Some(fut) = this.fut_ex.as_pin_mut() {
            let expect = ready!(fut
                .poll(cx)
                .map_err(|e| log::error!("Init http service error: {:?}", e)))?;
            this = self.as_mut().project();
            *this.expect = Some(expect);
            this.fut_ex.set(None);
        }

        if let Some(fut) = this.fut_upg.as_pin_mut() {
            let upgrade = ready!(fut
                .poll(cx)
                .map_err(|e| log::error!("Init http service error: {:?}", e)))?;
            this = self.as_mut().project();
            *this.upgrade = Some(upgrade);
            this.fut_ex.set(None);
        }

        let result = ready!(this
            .fut
            .poll(cx)
            .map_err(|e| log::error!("Init http service error: {:?}", e)));

        Poll::Ready(result.map(|service| {
            let this = self.as_mut().project();

            H1ServiceHandler::new(
                this.cfg.take().unwrap(),
                service,
                this.expect.take().unwrap(),
                this.upgrade.take(),
                this.on_connect.clone(),
                this.on_connect_ext.clone(),
            )
        }))
    }
}

/// `Service` implementation for HTTP/1 transport
pub struct H1ServiceHandler<T, S: Service, B, X: Service, U: Service>
where
    T: ServiceStream,
{
    srv: CloneableService<S>,
    expect: CloneableService<X>,
    upgrade: Option<CloneableService<U>>,
    on_connect: Option<Rc<dyn Fn(&T) -> Box<dyn DataFactory>>>,
    on_connect_ext: Option<Rc<ConnectCallback<T>>>,
    cfg: ServiceConfig<T::Runtime>,
    _t: PhantomData<B>,
}

impl<T, S, B, X, U> H1ServiceHandler<T, S, B, X, U>
where
    T: ServiceStream,
    S: Service<Request = Request>,
    S::Error: Into<Error>,
    S::Response: Into<Response<B>>,
    B: MessageBody,
    X: Service<Request = Request, Response = Request>,
    X::Error: Into<Error>,
    U: Service<Request = (Request, Framed<T, Codec<T::Runtime>>), Response = ()>,
    U::Error: fmt::Display,
{
    fn new(
        cfg: ServiceConfig<T::Runtime>,
        srv: S,
        expect: X,
        upgrade: Option<U>,
        on_connect: Option<Rc<dyn Fn(&T) -> Box<dyn DataFactory>>>,
        on_connect_ext: Option<Rc<ConnectCallback<T>>>,
    ) -> H1ServiceHandler<T, S, B, X, U> {
        H1ServiceHandler {
            srv: CloneableService::new(srv),
            expect: CloneableService::new(expect),
            upgrade: upgrade.map(CloneableService::new),
            cfg,
            on_connect,
            on_connect_ext,
            _t: PhantomData,
        }
    }
}

impl<T, S, B, X, U> Service for H1ServiceHandler<T, S, B, X, U>
where
    T: ServiceStream,
    S: Service<Request = Request>,
    S::Error: Into<Error>,
    S::Response: Into<Response<B>>,
    B: MessageBody,
    X: Service<Request = Request, Response = Request>,
    X::Error: Into<Error>,
    U: Service<Request = (Request, Framed<T, Codec<T::Runtime>>), Response = ()>,
    U::Error: fmt::Display + Into<Error>,
{
    type Request = (T, Option<net::SocketAddr>);
    type Response = ();
    type Error = DispatchError;
    type Future = Dispatcher<T, S, B, X, U>;

    fn poll_ready(&self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let ready = self
            .expect
            .poll_ready(cx)
            .map_err(|e| {
                let e = e.into();
                log::error!("Http service readiness error: {:?}", e);
                DispatchError::Service(e)
            })?
            .is_ready();

        let ready = self
            .srv
            .poll_ready(cx)
            .map_err(|e| {
                let e = e.into();
                log::error!("Http service readiness error: {:?}", e);
                DispatchError::Service(e)
            })?
            .is_ready()
            && ready;

        let ready = if let Some(ref upg) = self.upgrade {
            upg.poll_ready(cx)
                .map_err(|e| {
                    let e = e.into();
                    log::error!("Http service readiness error: {:?}", e);
                    DispatchError::Service(e)
                })?
                .is_ready()
                && ready
        } else {
            ready
        };

        if ready {
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    }

    fn call(&self, (io, addr): Self::Request) -> Self::Future {
        let deprecated_on_connect = self.on_connect.as_ref().map(|handler| handler(&io));

        let mut connect_extensions = Extensions::new();
        if let Some(ref handler) = self.on_connect_ext {
            // run on_connect_ext callback, populating connect extensions
            handler(&io, &mut connect_extensions);
        }

        Dispatcher::new(
            io,
            self.cfg.clone(),
            self.srv.clone(),
            self.expect.clone(),
            self.upgrade.clone(),
            deprecated_on_connect,
            connect_extensions,
            addr,
        )
    }
}

/// `ServiceFactory` implementation for `OneRequestService` service
#[derive(Default)]
pub struct OneRequest<T: ServiceStream> {
    config: ServiceConfig<T::Runtime>,
}

impl<T: ServiceStream> OneRequest<T> {
    /// Create new `H1SimpleService` instance.
    pub fn new() -> Self {
        OneRequest {
            config: ServiceConfig::default(),
        }
    }
}

impl<T: ServiceStream> ServiceFactory for OneRequest<T> {
    type Request = T;
    type Response = (Request, Framed<T, Codec<T::Runtime>>);
    type Error = ParseError;
    type Config = ();
    type Service = OneRequestService<T>;
    type InitError = ();
    type Future = Ready<Result<Self::Service, Self::InitError>>;

    fn new_service(&self, _: ()) -> Self::Future {
        ready(Ok(OneRequestService {
            config: self.config.clone(),
        }))
    }
}

/// `Service` implementation for HTTP1 transport. Reads one request and returns
/// request and framed object.
pub struct OneRequestService<T: ServiceStream> {
    config: ServiceConfig<T::Runtime>,
}

impl<T: ServiceStream> Service for OneRequestService<T> {
    type Request = T;
    type Response = (Request, Framed<T, Codec<T::Runtime>>);
    type Error = ParseError;
    type Future = impl Future<Output = Result<Self::Response, Self::Error>>;

    fn poll_ready(&self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&self, req: Self::Request) -> Self::Future {
        let mut framed = Framed::new(req, Codec::new(self.config.clone()));
        async move {
            match framed.next().await {
                Some(Ok(msg)) => match msg {
                    Message::Item(req) => Ok((req, framed)),
                    Message::Chunk(_) => unreachable!("Something is wrong"),
                },
                Some(Err(err)) => Err(err),
                None => Err(ParseError::Incomplete),
            }
        }
    }
}
