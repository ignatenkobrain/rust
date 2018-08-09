#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
use failure::{Backtrace, Context, Fail};
use serde_json;
use std::io::BufRead;
use std::sync::{Arc, RwLock};
use varlink::{self, CallTrait};
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Netdev {
    pub ifindex: i64,
    pub ifname: String,
}
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct NetdevInfo {
    pub ifindex: i64,
    pub ifname: String,
}
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Info_Reply {
    pub info: NetdevInfo,
}
impl varlink::VarlinkReply for Info_Reply {}
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Info_Args {
    pub ifindex: i64,
}
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct List_Reply {
    pub netdevs: Vec<Netdev>,
}
impl varlink::VarlinkReply for List_Reply {}
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct List_Args {}
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct UnknownError_Args {
    pub text: String,
}
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct UnknownNetworkIfIndex_Args {
    pub ifindex: i64,
}
pub trait VarlinkCallError: varlink::CallTrait {
    fn reply_unknown_error(&mut self, text: String) -> varlink::Result<()> {
        self.reply_struct(varlink::Reply::error(
            "io.systemd.network.UnknownError",
            Some(serde_json::to_value(UnknownError_Args { text })?),
        ))
    }
    fn reply_unknown_network_if_index(&mut self, ifindex: i64) -> varlink::Result<()> {
        self.reply_struct(varlink::Reply::error(
            "io.systemd.network.UnknownNetworkIfIndex",
            Some(serde_json::to_value(UnknownNetworkIfIndex_Args {
                ifindex,
            })?),
        ))
    }
}
impl<'a> VarlinkCallError for varlink::Call<'a> {}
#[derive(Debug)]
pub struct Error {
    inner: Context<ErrorKind>,
}
#[derive(Clone, PartialEq, Debug, Fail)]
pub enum ErrorKind {
    #[fail(display = "IO error")]
    Io_Error(::std::io::ErrorKind),
    #[fail(display = "(De)Serialization Error")]
    SerdeJson_Error(serde_json::error::Category),
    #[fail(display = "Varlink Error")]
    Varlink_Error(varlink::ErrorKind),
    #[fail(display = "Unknown error reply: '{:#?}'", _0)]
    VarlinkReply_Error(varlink::Reply),
    #[fail(display = "io.systemd.network.UnknownError: {:#?}", _0)]
    UnknownError(Option<UnknownError_Args>),
    #[fail(display = "io.systemd.network.UnknownNetworkIfIndex: {:#?}", _0)]
    UnknownNetworkIfIndex(Option<UnknownNetworkIfIndex_Args>),
}
impl Fail for Error {
    fn cause(&self) -> Option<&Fail> {
        self.inner.cause()
    }
    fn backtrace(&self) -> Option<&Backtrace> {
        self.inner.backtrace()
    }
}
impl ::std::fmt::Display for Error {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        ::std::fmt::Display::fmt(&self.inner, f)
    }
}
impl Error {
    #[allow(dead_code)]
    pub fn kind(&self) -> ErrorKind {
        self.inner.get_context().clone()
    }
}
impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Error {
        Error {
            inner: Context::new(kind),
        }
    }
}
impl From<Context<ErrorKind>> for Error {
    fn from(inner: Context<ErrorKind>) -> Error {
        Error { inner }
    }
}
impl From<::std::io::Error> for Error {
    fn from(e: ::std::io::Error) -> Error {
        let kind = e.kind();
        e.context(ErrorKind::Io_Error(kind)).into()
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Error {
        let cat = e.classify();
        e.context(ErrorKind::SerdeJson_Error(cat)).into()
    }
}
#[allow(dead_code)]
pub type Result<T> = ::std::result::Result<T, Error>;
impl From<varlink::Error> for Error {
    fn from(e: varlink::Error) -> Self {
        match &e.kind() {
            varlink::ErrorKind::Io(kind) => e.context(ErrorKind::Io_Error(kind)).into(),
            varlink::ErrorKind::SerdeJsonSer(cat) => {
                e.context(ErrorKind::SerdeJson_Error(cat)).into()
            }
            kind => e.context(ErrorKind::Varlink_Error(kind)).into(),
        }
    }
}
impl From<varlink::Reply> for Error {
    fn from(e: varlink::Reply) -> Self {
        if varlink::Error::is_error(&e) {
            return varlink::Error::from(e).into();
        }
        match e {
            varlink::Reply {
                error: Some(ref t), ..
            } if t == "io.systemd.network.UnknownError" =>
            {
                match e {
                    varlink::Reply {
                        parameters: Some(p),
                        ..
                    } => match serde_json::from_value(p) {
                        Ok(v) => ErrorKind::UnknownError(v).into(),
                        Err(_) => ErrorKind::UnknownError(None).into(),
                    },
                    _ => ErrorKind::UnknownError(None).into(),
                }
            }
            varlink::Reply {
                error: Some(ref t), ..
            } if t == "io.systemd.network.UnknownNetworkIfIndex" =>
            {
                match e {
                    varlink::Reply {
                        parameters: Some(p),
                        ..
                    } => match serde_json::from_value(p) {
                        Ok(v) => ErrorKind::UnknownNetworkIfIndex(v).into(),
                        Err(_) => ErrorKind::UnknownNetworkIfIndex(None).into(),
                    },
                    _ => ErrorKind::UnknownNetworkIfIndex(None).into(),
                }
            }
            _ => ErrorKind::VarlinkReply_Error(e).into(),
        }
    }
}
