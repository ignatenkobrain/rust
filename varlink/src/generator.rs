//! Generate rust code from varlink interface definition files

use failure::{Backtrace, Context, Fail};
use proc_macro2::{Ident, Span, TokenStream};
use std::borrow::Cow;
use std::env;
use std::fmt::{self, Display};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, exit};
use std::str::FromStr;
use varlink_parser::{
    self, Method, Typedef, Varlink, VEnum, VError, VStruct, VStructOrEnum, VType, VTypeExt,
};

macro_rules! quote_cs {
    ($($tt:tt)*) => (quote_spanned!(::proc_macro2::Span::call_site()=>
                                    $($tt)*))
}

#[derive(Debug)]
pub struct Error {
    inner: Context<ErrorKind>,
}

#[derive(Clone, PartialEq, Debug, Fail)]
pub enum ErrorKind {
    #[fail(display = "IO error")]
    Io,
    #[fail(display = "Parse Error")]
    Parser,
}

impl Fail for Error {
    fn cause(&self) -> Option<&Fail> {
        self.inner.cause()
    }

    fn backtrace(&self) -> Option<&Backtrace> {
        self.inner.backtrace()
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        Display::fmt(&self.inner, f)
    }
}

impl Error {
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
        e.context(ErrorKind::Io).into()
    }
}

impl From<varlink_parser::Error> for Error {
    fn from(e: varlink_parser::Error) -> Error {
        e.context(ErrorKind::Parser).into()
    }
}

pub type Result<T> = ::std::result::Result<T, Error>;

trait ToRustString<'short, 'long: 'short> {
    fn to_rust_string(
        &'long self,
        name: &str,
        tokenstream: &mut TokenStream,
        options: &'long GeneratorOptions,
    ) -> Cow<'long, str>;
}

trait ToTokenStream<'short, 'long: 'short> {
    fn to_tokenstream(
        &'long self,
        name: &str,
        tokenstream: &mut TokenStream,
        options: &'long GeneratorOptions,
    );
}

#[derive(Default)]
pub struct GeneratorOptions {
    pub bool_type: Option<&'static str>,
    pub int_type: Option<&'static str>,
    pub float_type: Option<&'static str>,
    pub string_type: Option<&'static str>,
    pub preamble: Option<TokenStream>,
}

impl<'short, 'long: 'short> ToRustString<'short, 'long> for VType<'long> {
    fn to_rust_string(
        &'long self,
        name: &str,
        tokenstream: &mut TokenStream,
        options: &'long GeneratorOptions,
    ) -> Cow<'long, str> {
        match *self {
            VType::Bool => options.bool_type.unwrap_or("bool").into(),
            VType::Int => options.int_type.unwrap_or("i64").into(),
            VType::Float => options.float_type.unwrap_or("f64").into(),
            VType::String => options.string_type.unwrap_or("String").into(),
            VType::Object => "serde_json::Value".into(),
            VType::Typename(v) => v.into(),
            VType::Enum(ref v) => {
                v.to_tokenstream(name, tokenstream, options);
                Cow::Owned(name.to_string())
            }
            VType::Struct(ref v) => {
                v.to_tokenstream(name, tokenstream, options);
                Cow::Owned(name.to_string())
            }
        }
    }
}

impl<'short, 'long: 'short> ToRustString<'short, 'long> for VTypeExt<'long> {
    fn to_rust_string(
        &'long self,
        name: &str,
        tokenstream: &mut TokenStream,
        options: &'long GeneratorOptions,
    ) -> Cow<'long, str> {
        match *self {
            VTypeExt::Plain(ref vtype) => vtype.to_rust_string(name, tokenstream, options),
            VTypeExt::Array(ref v) => {
                format!("Vec<{}>", v.to_rust_string(name, tokenstream, options)).into()
            }
            VTypeExt::Dict(ref v) => match *v.as_ref() {
                VTypeExt::Plain(VType::Struct(ref s)) if s.elts.is_empty() => {
                    "varlink::StringHashSet".into()
                }
                _ => format!(
                    "varlink::StringHashMap<{}>",
                    v.to_rust_string(name, tokenstream, options)
                ).into(),
            },
            VTypeExt::Option(ref v) => {
                format!("Option<{}>", v.to_rust_string(name, tokenstream, options)).into()
            }
        }
    }
}

fn to_snake_case(mut str: &str) -> String {
    let mut words = vec![];
    // Preserve leading underscores
    str = str.trim_left_matches(|c: char| {
        if c == '_' {
            words.push(String::new());
            true
        } else {
            false
        }
    });
    for s in str.split('_') {
        let mut last_upper = false;
        let mut buf = String::new();
        if s.is_empty() {
            continue;
        }
        for ch in s.chars() {
            if !buf.is_empty() && buf != "'" && ch.is_uppercase() && !last_upper {
                words.push(buf);
                buf = String::new();
            }
            last_upper = ch.is_uppercase();
            buf.extend(ch.to_lowercase());
        }
        words.push(buf);
    }
    words.join("_")
}

fn is_rust_keyword(v: &str) -> bool {
    match v {
        | "abstract" | "as" | "async" | "auto" | "become" | "box" | "break" | "catch" | "const"
        | "continue" | "crate" | "default" | "do" | "dyn" | "else" | "enum" | "extern"
        | "false" | "final" | "fn" | "for" | "if" | "impl" | "in" | "let" | "loop" | "macro"
        | "match" | "mod" | "move" | "mut" | "override" | "priv" | "pub" | "ref" | "return"
        | "Self" | "self" | "static" | "struct" | "super" | "trait" | "true" | "type"
        | "typeof" | "union" | "unsafe" | "unsized" | "use" | "virtual" | "where" | "while"
        | "yield" => true,
        _ => false,
    }
}

fn replace_if_rust_keyword(v: &str) -> String {
    if is_rust_keyword(v) {
        String::from(v) + "_"
    } else {
        String::from(v)
    }
}

fn replace_if_rust_keyword_annotate2(v: &str) -> (String, TokenStream) {
    if is_rust_keyword(v) {
        (
            String::from(v) + "_",
            TokenStream::from_str(format!(" #[serde(rename = \"{}\")]", v).as_ref()).unwrap(),
        )
    } else {
        (String::from(v), TokenStream::new())
    }
}

impl<'short, 'long: 'short> ToTokenStream<'short, 'long> for VStruct<'long> {
    fn to_tokenstream(
        &'long self,
        name: &str,
        tokenstream: &mut TokenStream,
        options: &'long GeneratorOptions,
    ) {
        let tname = Ident::new(replace_if_rust_keyword(name).as_ref(), Span::call_site());

        let mut enames = vec![];
        let mut etypes = vec![];
        let mut anot = vec![];
        for e in &self.elts {
            let (ename, tt) = replace_if_rust_keyword_annotate2(e.name);
            anot.push(tt);
            enames.push(Ident::new(&ename, Span::call_site()));
            etypes.push(
                TokenStream::from_str(
                    e.vtype
                        .to_rust_string(
                            format!("{}_{}", name, e.name).as_ref(),
                            tokenstream,
                            options,
                        )
                        .as_ref(),
                ).unwrap(),
            );
        }
        tokenstream.extend(quote_cs!(
                    #[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
                    pub struct #tname {
                        #(#anot pub #enames: #etypes,)*
                    }
                    ));
    }
}

impl<'short, 'long: 'short> ToTokenStream<'short, 'long> for VEnum<'long> {
    fn to_tokenstream(
        &'long self,
        name: &str,
        tokenstream: &mut TokenStream,
        _options: &'long GeneratorOptions,
    ) {
        let tname = Ident::new(replace_if_rust_keyword(name).as_ref(), Span::call_site());

        let mut enames = vec![];
        let mut anot = vec![];

        for elt in &self.elts {
            let (ename, tt) = replace_if_rust_keyword_annotate2(elt);
            anot.push(tt);
            enames.push(Ident::new(&ename, Span::call_site()));
        }
        tokenstream.extend(quote_cs!(
                #[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
                pub enum #tname {
                    #(#anot #enames, )*
                }
                ));
    }
}

impl<'short, 'long: 'short> ToTokenStream<'short, 'long> for Typedef<'long> {
    fn to_tokenstream(
        &'long self,
        _name: &str,
        tokenstream: &mut TokenStream,
        options: &'long GeneratorOptions,
    ) {
        match self.elt {
            VStructOrEnum::VStruct(ref v) => v.to_tokenstream(self.name, tokenstream, options),
            VStructOrEnum::VEnum(ref v) => v.to_tokenstream(self.name, tokenstream, options),
        }
    }
}

impl<'short, 'long: 'short> ToTokenStream<'short, 'long> for VError<'long> {
    fn to_tokenstream(
        &'long self,
        _name: &str,
        tokenstream: &mut TokenStream,
        options: &'long GeneratorOptions,
    ) {
        let args_name = Ident::new(&format!("{}_Args", self.name), Span::call_site());
        let mut args_enames = vec![];
        let mut args_etypes = vec![];
        let mut args_anot = vec![];
        for e in &self.parm.elts {
            let mut a = if let VTypeExt::Option(_) = e.vtype {
                quote_cs!(#[serde(skip_serializing_if = "Option::is_none")])
            } else {
                quote_cs!()
            };
            let (ename, tt) = replace_if_rust_keyword_annotate2(e.name);
            a.extend(tt);
            args_anot.push(a);
            args_enames.push(Ident::new(&ename, Span::call_site()));
            args_etypes.push(
                TokenStream::from_str(
                    e.vtype
                        .to_rust_string(
                            format!("{}_Args_{}", self.name, e.name).as_ref(),
                            tokenstream,
                            options,
                        )
                        .as_ref(),
                ).unwrap(),
            );
        }
        tokenstream.extend(quote_cs!(
        #[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
        pub struct #args_name {
                        #(#args_anot pub #args_enames: #args_etypes,)*
        }

        ));
    }
}

impl<'short, 'long: 'short> ToTokenStream<'short, 'long> for Method<'long> {
    fn to_tokenstream(
        &'long self,
        _name: &str,
        tokenstream: &mut TokenStream,
        options: &'long GeneratorOptions,
    ) {
        let reply_name = Ident::new(&format!("{}_Reply", self.name), Span::call_site());
        let mut reply_enames = vec![];
        let mut reply_etypes = vec![];
        let mut reply_anot = vec![];
        for e in &self.output.elts {
            let mut a = if let VTypeExt::Option(_) = e.vtype {
                quote_cs!(#[serde(skip_serializing_if = "Option::is_none")])
            } else {
                quote_cs!()
            };
            let (ename, tt) = replace_if_rust_keyword_annotate2(e.name);
            a.extend(tt);
            reply_anot.push(a);
            reply_enames.push(Ident::new(&ename, Span::call_site()));
            reply_etypes.push(
                TokenStream::from_str(
                    e.vtype
                        .to_rust_string(
                            format!("{}_Reply_{}", self.name, e.name).as_ref(),
                            tokenstream,
                            options,
                        )
                        .as_ref(),
                ).unwrap(),
            );
        }

        let args_name = Ident::new(&format!("{}_Args", self.name), Span::call_site());
        let mut args_enames = vec![];
        let mut args_etypes = vec![];
        let mut args_anot = vec![];
        for e in &self.input.elts {
            let mut a = if let VTypeExt::Option(_) = e.vtype {
                quote_cs!(#[serde(skip_serializing_if = "Option::is_none")])
            } else {
                quote_cs!()
            };
            let (ename, tt) = replace_if_rust_keyword_annotate2(e.name);
            a.extend(tt);
            args_anot.push(a);
            args_enames.push(Ident::new(&ename, Span::call_site()));
            args_etypes.push(
                TokenStream::from_str(
                    e.vtype
                        .to_rust_string(
                            format!("{}_Args_{}", self.name, e.name).as_ref(),
                            tokenstream,
                            options,
                        )
                        .as_ref(),
                ).unwrap(),
            );
        }
        tokenstream.extend(quote_cs!(
        #[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
        pub struct #reply_name {
                        #(#reply_anot pub #reply_enames: #reply_etypes,)*
        }

        impl varlink::VarlinkReply for #reply_name {}

        #[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
        pub struct #args_name {
                        #(#args_anot pub #args_enames: #args_etypes,)*
        }

        ));
    }
}

fn varlink_to_rust(
    varlink: &Varlink,
    options: &GeneratorOptions,
    tosource: bool,
) -> Result<TokenStream> {
    let iface = &varlink.interface;
    let mut ts = TokenStream::new();

    if tosource {
        ts.extend(quote_cs!(
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
            ));
    }

    ts.extend(quote_cs!(
use failure::{{Backtrace, Context, Fail}};
use serde_json;
use std::io::BufRead;
use std::sync::{{Arc, RwLock}};
use varlink::{{self, CallTrait}};
));

    if let Some(ref v) = options.preamble {
        ts.extend(v.clone());
    }

    for t in iface.typedefs.values() {
        t.to_tokenstream("", &mut ts, options);
    }

    for t in iface.methods.values() {
        t.to_tokenstream("", &mut ts, options);
    }

    for t in iface.errors.values() {
        t.to_tokenstream("", &mut ts, options);
    }

    {
        let mut funcs = TokenStream::new();
        for t in iface.errors.values() {
            let mut inparms_name = Vec::new();
            let mut inparms_type = Vec::new();
            let mut innames = Vec::new();

            let inparms;
            let parms;
            let args_name = Ident::new(&format!("{}_Args", t.name), Span::call_site());
            if !t.parm.elts.is_empty() {
                for e in &t.parm.elts {
                    let ident = Ident::new(&replace_if_rust_keyword(e.name), Span::call_site());
                    inparms_name.push(ident.clone());
                    inparms_type.push(
                        TokenStream::from_str(
                            e.vtype
                                .to_rust_string(
                                    format!("{}_Args_{}", t.name, e.name).as_ref(),
                                    &mut funcs,
                                    options,
                                )
                                .as_ref(),
                        ).unwrap(),
                    );
                    innames.push(ident);
                }
                inparms = quote_cs!(#(#inparms_name : #inparms_type),*);
                parms = quote_cs!(Some(serde_json::to_value(#args_name {#(#innames),*})?));
            } else {
                parms = quote_cs!(None);
                inparms = quote_cs!();
            }
            let errorname = format!("{iname}.{ename}", iname = iface.name, ename = t.name);
            let func_name = Ident::new(
                &format!("reply_{}", to_snake_case(t.name)),
                Span::call_site(),
            );

            funcs.extend(quote_cs!(
                    fn #func_name(&mut self, #inparms) -> varlink::Result<()> {
                        self.reply_struct(varlink::Reply::error(#errorname, #parms))
                    }
                ));
        }
        ts.extend(quote_cs!(
            pub trait VarlinkCallError: varlink::CallTrait {
                #funcs
            }
        ));
    }

    ts.extend(quote_cs!(
        impl<'a> VarlinkCallError for varlink::Call<'a> {}

        #[derive(Debug)]
        pub struct Error {
            inner: Context<ErrorKind>,
        }
    ));

    {
        let mut errors = Vec::new();
        for t in iface.errors.values() {
            errors.push(TokenStream::from_str(
                &format!(
                    "#[fail(display = \"{iname}.{ename}: {{:#?}}\", _0)]\
                 {ename}(Option<{ename}_Args>)",
                    ename = t.name,
                    iname = iface.name,
                )).unwrap());
        }

        ts.extend(quote_cs!(
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
                #(#errors),*
            }
        ));
    }

    ts.extend(quote_cs!(
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
                varlink::ErrorKind::SerdeJsonSer(cat) => e.context(ErrorKind::SerdeJson_Error(cat)).into(),
                kind => e.context(ErrorKind::Varlink_Error(kind)).into(),
            }
        }
    }
        ));

    {
        let mut arms = TokenStream::new();
        for t in iface.errors.values() {
            let error_name = format!("{iname}.{ename}", iname = iface.name,
                                     ename = t.name);
            let ename = TokenStream::from_str(&format!("ErrorKind::{}", t.name)).unwrap();
            arms.extend(quote_cs!(
                varlink::Reply { error: Some(ref t), .. } if t == #error_name => {
                    match e {
                       varlink::Reply {
                           parameters: Some(p),
                           ..
                       } => match serde_json::from_value(p) {
                           Ok(v) => #ename(v).into(),
                           Err(_) => #ename(None).into(),
                       },
                       _ => #ename(None).into(),
                    }
                }
            ));
        }


        ts.extend(quote_cs!(
            impl From<varlink::Reply> for Error {
                fn from(e: varlink::Reply) -> Self {
                    if varlink::Error::is_error(&e) {
                        return varlink::Error::from(e).into();
                    }

                    match e {
                    #arms
                    _ => ErrorKind::VarlinkReply_Error(e).into(),
                    }
                }
            }
        ));
    }

    /*
    for t in iface.methods.values() {
        let mut inparms = String::new();
        let mut innames = String::new();
        if !t.output.elts.is_empty() {
            for e in &t.output.elts {
                inparms += format!(
                    ", {}: {}",
                    replace_if_rust_keyword(e.name),
                    e.vtype.to_rust_string(
                        format!("{}_Reply_{}", t.name, e.name).as_ref(),
                        &mut enumvec,
                        &mut structvec,
                        options,
                    )
                ).as_ref();
                innames += format!("{}, ", replace_if_rust_keyword(e.name)).as_ref();
            }
            innames.pop();
            innames.pop();
        }
        write!(w, "pub trait Call_{}: VarlinkCallError {{\n", t.name)?;
        write!(
            w,
            "    fn reply(&mut self{}) -> varlink::Result<()> {{\n",
            inparms
        )?;
        if !t.output.elts.is_empty() {
            write!(
                w,
                "        self.reply_struct({}_Reply {{ {} }}.into())\n",
                t.name, innames
            )?;
        } else {
            write!(
                w,
                "        self.reply_struct(varlink::Reply::parameters(None))\n"
            )?;
        }
        write!(
            w,
            "    }}\n}}\n\nimpl<'a> Call_{} for varlink::Call<'a> {{}}\n\n",
            t.name
        )?;
    }

    write!(w, "pub trait VarlinkInterface {{\n")?;
    for t in iface.methods.values() {
        let mut inparms = String::new();
        if !t.input.elts.is_empty() {
            for e in &t.input.elts {
                inparms += format!(
                    ", {}: {}",
                    replace_if_rust_keyword(e.name),
                    e.vtype.to_rust_string(
                        format!("{}_Args_{}", t.name, e.name).as_ref(),
                        &mut enumvec,
                        &mut structvec,
                        options,
                    )
                ).as_ref();
            }
        }

        write!(
            w,
            "    fn {}(&self, call: &mut Call_{}{}) -> varlink::Result<()>;\n",
            to_snake_case(t.name),
            t.name,
            inparms
        )?;
    }

    write!(
        w,
        r#"    fn call_upgraded(&self, _call: &mut varlink::Call, _bufreader: &mut BufRead) ->
        varlink::Result<Vec<u8>> {{
        Ok(Vec::new())
    }}
}}

"#
    )?;

    write!(w, "pub trait VarlinkClientInterface {{\n")?;
    for t in iface.methods.values() {
        let mut inparms = String::new();
        let mut outparms = String::new();
        if !t.input.elts.is_empty() {
            for e in &t.input.elts {
                inparms += format!(
                    ", {}: {}",
                    replace_if_rust_keyword(e.name),
                    e.vtype.to_rust_string(
                        format!("{}_Args_{}", t.name, e.name).as_ref(),
                        &mut enumvec,
                        &mut structvec,
                        options,
                    )
                ).as_ref();
            }
        }
        if !t.output.elts.is_empty() {
            for e in &t.output.elts {
                outparms += format!(
                    "{}, ",
                    e.vtype.to_rust_string(
                        format!("{}_Reply_{}", t.name, e.name).as_ref(),
                        &mut enumvec,
                        &mut structvec,
                        options,
                    )
                ).as_ref();
            }
            outparms.pop();
            outparms.pop();
        }

        write!(
            w,
            "    fn {sname}(&mut self{inparms}) -> varlink::MethodCall<{mname}_Args, \
             {mname}_Reply, Error>;\
             \n",
            sname = to_snake_case(t.name),
            inparms = inparms,
            mname = t.name
        )?;
    }

    write!(w, "}}\n")?;

    write!(
        w,
        r#"
#[allow(dead_code)]
pub struct VarlinkClient {{
    connection: Arc<RwLock<varlink::Connection>>,
}}

impl VarlinkClient {{
    #[allow(dead_code)]
    pub fn new(connection: Arc<RwLock<varlink::Connection>>) -> Self {{
        VarlinkClient {{
            connection,
        }}
    }}
}}

impl VarlinkClientInterface for VarlinkClient {{
"#
    )?;
    for t in iface.methods.values() {
        let mut inparms = String::new();
        let mut innames = String::new();
        if !t.input.elts.is_empty() {
            for e in &t.input.elts {
                inparms += format!(
                    ", {}: {}",
                    replace_if_rust_keyword(e.name),
                    e.vtype.to_rust_string(
                        format!("{}_Args_{}", t.name, e.name).as_ref(),
                        &mut enumvec,
                        &mut structvec,
                        options,
                    )
                ).as_ref();
                innames += format!("{}, ", replace_if_rust_keyword(e.name)).as_ref();
            }
            innames.pop();
            innames.pop();
        }
        write!(
            w,
            "    fn {sname}(&mut self{inparms}) -> varlink::MethodCall<{mname}_Args, \
             {mname}_Reply, \
             Error> \
             {{\n",
            sname = to_snake_case(t.name),
            inparms = inparms,
            mname = t.name
        )?;

        write!(
            w,
            "            \
             varlink::MethodCall::<{mname}_Args, {mname}_Reply, Error>::new(\n            \
             self.connection.clone(),\n            \
             \"{iname}.{mname}\",\n            \
             {mname}_Args {{ {innames} }},\n        \
             )\n",
            mname = t.name,
            iname = iface.name,
            innames = innames
        )?;
        write!(w, "    }}\n")?;
    }
    write!(w, "}}\n")?;

    write!(
        w,
        r########################################################################################"
#[allow(dead_code)]
pub struct VarlinkInterfaceProxy {{
    inner: Box<VarlinkInterface + Send + Sync>,
}}

#[allow(dead_code)]
pub fn new(inner: Box<VarlinkInterface + Send + Sync>) -> VarlinkInterfaceProxy {{
    VarlinkInterfaceProxy {{ inner }}
}}

impl varlink::Interface for VarlinkInterfaceProxy {{
    fn get_description(&self) -> &'static str {{
        r#####################################"{description}"#####################################
    }}

    fn get_name(&self) -> &'static str {{
        "{iname}"
    }}

"########################################################################################,
        description = varlink.description,
        iname = iface.name
    )?;

    write!(
        w,
        r#"    fn call_upgraded(&self, call: &mut varlink::Call, bufreader: &mut BufRead) ->
        varlink::Result<Vec<u8>> {{
        self.inner.call_upgraded(call, bufreader)
    }}

    fn call(&self, call: &mut varlink::Call) -> varlink::Result<()> {{
        let req = call.request.unwrap();
        match req.method.as_ref() {{
"#
    )?;

    for t in iface.methods.values() {
        let mut inparms = String::new();
        for e in &t.input.elts {
            inparms += format!(", args.{}", replace_if_rust_keyword(e.name)).as_ref();
        }

        write!(
            w,
            "            \"{iname}.{mname}\" => {{",
            iname = iface.name,
            mname = t.name
        )?;
        if !t.input.elts.is_empty() {
            write!(
                w,
                concat!(
                    "\n",
                    "                if let Some(args) = req.parameters.clone() {{\n",
                    "                    let args: {mname}_Args = match serde_json::from_value(args) {{\
                     Ok(v) => v, Err(e) => {{ let es = format!(\"{{}}\", e); let _ = call\
                     .reply_invalid_parameter(es.clone()); return \
                      Err(varlink::ErrorKind::SerdeJsonDe(es).into()); }} }};\n",
                    "                    self.inner.{sname}(call as &mut Call_{mname}{inparms})\n",
                    "                }} else {{\n",
                    "                    call.reply_invalid_parameter(\"parameters\".into())\
                     \n",
                    "                }}\n",
                    "            }}\n"
                ),
                mname = t.name,
                sname = to_snake_case(t.name),
                inparms = inparms
            )?;
        } else {
            write!(
                w,
                concat!(
                    "\n",
                    "                self.inner.{sname}(call as &mut Call_{mname})\n",
                    "            }}\n"
                ),
                sname = to_snake_case(t.name),
                mname = t.name
            )?;
        }
    }
    write!(
        w,
        concat!(
            "\n",
            "            m => {{\n",
            "                call.reply_method_not_found(String::from(m))\n",
            "            }}\n",
            "        }}\n",
            "    }}\n",
            "}}"
        )
    )?;
*/

    Ok(ts)
}

/// `generate` reads a varlink interface definition from `reader` and writes
/// the rust code to `writer`.
pub fn generate(reader: &mut Read, writer: &mut Write, tosource: bool) -> Result<()> {
    generate_with_options(
        reader,
        writer,
        &GeneratorOptions {
            ..Default::default()
        },
        tosource,
    )
}

/// `generate_with_options` reads a varlink interface definition from `reader` and writes
/// the rust code to `writer`.
pub fn generate_with_options(
    reader: &mut Read,
    writer: &mut Write,
    options: &GeneratorOptions,
    tosource: bool,
) -> Result<()> {
    let mut buffer = String::new();

    reader.read_to_string(&mut buffer)?;

    let vr = Varlink::from_string(&buffer)?;

    let ts = varlink_to_rust(&vr, options, tosource)?;
    writer.write(ts.to_string().as_bytes())?;
    Ok(())
}

/// cargo build helper function
///
/// `cargo_build` is used in a `build.rs` program to build the rust code
/// from a varlink interface definition.
///
/// Errors are emitted to stderr and terminate the process.
///
///# Examples
///
///```rust,no_run
///extern crate varlink;
///
///fn main() {
///    varlink::generator::cargo_build("src/org.example.ping.varlink");
///}
///```
///
pub fn cargo_build<T: AsRef<Path> + ?Sized>(input_path: &T) {
    cargo_build_options(
        input_path,
        &GeneratorOptions {
            ..Default::default()
        },
    )
}

/// cargo build helper function
///
/// `cargo_build` is used in a `build.rs` program to build the rust code
/// from a varlink interface definition.
///
/// Errors are emitted to stderr and terminate the process.
///
///# Examples
///
///```rust,no_run
///extern crate varlink;
///
///fn main() {
///    varlink::generator::cargo_build_options("src/org.example.ping.varlink",
///       &varlink::generator::GeneratorOptions {
///           int_type: Some("i128"),
///            ..Default::default()
///        });
///}
///```
///
pub fn cargo_build_options<T: AsRef<Path> + ?Sized>(input_path: &T, options: &GeneratorOptions) {
    let input_path = input_path.as_ref();

    let out_dir: PathBuf = env::var_os("OUT_DIR").unwrap().into();
    let rust_path = out_dir
        .join(input_path.file_name().unwrap())
        .with_extension("rs");

    let writer: &mut Write = &mut (File::create(&rust_path).unwrap_or_else(|e| {
        eprintln!(
            "Could not open varlink output file `{}`: {}",
            rust_path.display(),
            e
        );
        exit(1);
    }));

    let reader: &mut Read = &mut (File::open(input_path).unwrap_or_else(|e| {
        eprintln!(
            "Could not read varlink input file `{}`: {}",
            input_path.display(),
            e
        );
        exit(1);
    }));

    if let Err(e) = generate_with_options(reader, writer, options, false) {
        eprintln!(
            "Could not generate rust code from varlink file `{}`: {}",
            input_path.display(),
            e
        );
        exit(1);
    }

    println!("cargo:rerun-if-changed={}", input_path.display());
}

/// cargo build helper function
///
/// `cargo_build_tosource` is used in a `build.rs` program to build the rust code
/// from a varlink interface definition. This function saves the rust code
/// in the same directory as the varlink file. The name is the name of the varlink file
/// and "." replaced with "_" and of course ending with ".rs".
///
/// Use this, if you are using an IDE with code completion, as most cannot cope with
/// `include!(concat!(env!("OUT_DIR"), "<varlink_file>"));`
///
/// Set `rustfmt` to `true`, if you want the generator to run rustfmt on the generated
/// code. This might be good practice to avoid large changes after a global `cargo fmt` run.
///
/// Errors are emitted to stderr and terminate the process.
///
///# Examples
///
///```rust,no_run
///extern crate varlink;
///
///fn main() {
///    varlink::generator::cargo_build_tosource("src/org.example.ping.varlink", true);
///}
///```
///
pub fn cargo_build_tosource<T: AsRef<Path> + ?Sized>(input_path: &T, rustfmt: bool) {
    cargo_build_tosource_options(
        input_path,
        rustfmt,
        &GeneratorOptions {
            ..Default::default()
        },
    )
}

/// cargo build helper function
///
/// `cargo_build_tosource_options` is used in a `build.rs` program to build the rust code
/// from a varlink interface definition. This function saves the rust code
/// in the same directory as the varlink file. The name is the name of the varlink file
/// and "." replaced with "_" and of course ending with ".rs".
///
/// Use this, if you are using an IDE with code completion, as most cannot cope with
/// `include!(concat!(env!("OUT_DIR"), "<varlink_file>"));`
///
/// Set `rustfmt` to `true`, if you want the generator to run rustfmt on the generated
/// code. This might be good practice to avoid large changes after a global `cargo fmt` run.
///
/// Errors are emitted to stderr and terminate the process.
///
///# Examples
///
///```rust,no_run
///extern crate varlink;
///
///fn main() {
///    varlink::generator::cargo_build_tosource_options("src/org.example.ping.varlink", true,
///        &varlink::generator::GeneratorOptions {
///           int_type: Some("i128"),
///            ..Default::default()
///        }
///    );
///}
///```
///
pub fn cargo_build_tosource_options<T: AsRef<Path> + ?Sized>(
    input_path: &T,
    rustfmt: bool,
    options: &GeneratorOptions,
) {
    let input_path = input_path.as_ref();
    let noextension = input_path.with_extension("");
    let newfilename = noextension
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .replace(".", "_");
    let rust_path = input_path
        .parent()
        .unwrap()
        .join(Path::new(&newfilename).with_extension("rs"));

    let writer: &mut Write = &mut (File::create(&rust_path).unwrap_or_else(|e| {
        eprintln!(
            "Could not open varlink output file `{}`: {}",
            rust_path.display(),
            e
        );
        exit(1);
    }));

    let reader: &mut Read = &mut (File::open(input_path).unwrap_or_else(|e| {
        eprintln!(
            "Could not read varlink input file `{}`: {}",
            input_path.display(),
            e
        );
        exit(1);
    }));

    if let Err(e) = generate_with_options(reader, writer, options, true) {
        eprintln!(
            "Could not generate rust code from varlink file `{}`: {}",
            input_path.display(),
            e
        );
        exit(1);
    }

    if rustfmt {
        if let Err(e) = Command::new("rustfmt")
            .arg(rust_path.to_str().unwrap())
            .output()
            {
                eprintln!(
                    "Could not run rustfmt on file `{}` {}",
                    rust_path.display(),
                    e
                );
                exit(1);
            }
    }

    println!("cargo:rerun-if-changed={}", input_path.display());
}
