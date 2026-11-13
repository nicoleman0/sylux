//! Strict process configuration (s1-design-review §8 source layout).
//!
//! No CLI-framework dependency: the accepted surface is exactly the reviewed
//! flags below and anything else refuses startup. Secret values are never
//! accepted as arguments or environment variables. The PoC boundary
//! (docs/poc-milestone.md) keeps loopback the default and allows non-loopback
//! binds only through an explicit flag, and only onto private addresses.

use std::{
    env,
    ffi::OsString,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    /// Explicit one-time host initialization of an empty dedicated root.
    Init,
    /// Serve the bound installation (never initializes anything).
    Serve,
    /// Reset an existing administrator password while the service is stopped.
    AdminResetPassword,
}

/// Parsed process configuration.
#[derive(Debug)]
pub struct CliConfig {
    pub command: CommandKind,
    pub root: PathBuf,
    pub listen: SocketAddr,
    /// Required for `serve`; accepted and unused by `init`.
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    pub allow_non_loopback: bool,
    /// Optional externally visible HTTPS origin used for CSRF checks.
    pub public_origin: Option<String>,
    /// Compiled static assets directory (environment override only).
    pub assets: PathBuf,
    pub admin_handle: Option<String>,
}

/// Why the configuration was refused. Redacted codes only: never echo
/// argument values back.
#[derive(Debug)]
pub enum ConfigError {
    Usage(&'static str),
}

impl ConfigError {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Usage(code) => code,
        }
    }
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "usage: {}", self.reason())
    }
}

impl std::error::Error for ConfigError {}

/// Parse subcommand arguments (excluding the program name). Environment
/// fallbacks: `SYLUX_LISTEN` (listen address) and `SYLUX_ASSETS` (static
/// assets), matching the H0 scaffold surface.
pub fn parse<I>(arguments: I) -> Result<CliConfig, ConfigError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut arguments = arguments.into_iter();
    let command = match arguments.next().as_deref().and_then(|value| value.to_str()) {
        Some("init") => CommandKind::Init,
        Some("serve") => CommandKind::Serve,
        Some("admin") => {
            if arguments.next().as_deref().and_then(|value| value.to_str())
                != Some("reset-password")
            {
                return Err(ConfigError::Usage("admin_subcommand_required"));
            }
            CommandKind::AdminResetPassword
        }
        _ => return Err(ConfigError::Usage("subcommand_required")),
    };
    let mut admin_handle = None;
    let mut password_stdin = false;
    let mut root: Option<PathBuf> = None;
    let mut listen: Option<String> = None;
    let mut tls_cert: Option<PathBuf> = None;
    let mut tls_key: Option<PathBuf> = None;
    let mut allow_non_loopback = false;
    let mut public_origin: Option<String> = None;
    while let Some(argument) = arguments.next() {
        let flag = argument
            .to_str()
            .ok_or(ConfigError::Usage("argument_not_utf8"))?;
        if command == CommandKind::AdminResetPassword
            && !matches!(flag, "--root" | "--handle" | "--password-stdin")
        {
            return Err(ConfigError::Usage("unknown_argument"));
        }
        match flag {
            "--handle" if command == CommandKind::AdminResetPassword => {
                set_once(
                    &mut admin_handle,
                    value_string(&mut arguments, "handle_value_missing")?,
                    "duplicate_handle",
                )?;
            }
            "--password-stdin" if command == CommandKind::AdminResetPassword => {
                if password_stdin {
                    return Err(ConfigError::Usage("duplicate_password_stdin"));
                }
                password_stdin = true;
            }
            "--root" => {
                set_once(
                    &mut root,
                    value_path(&mut arguments, "root_value_missing")?,
                    "duplicate_root",
                )?;
            }
            "--listen" => {
                set_once(
                    &mut listen,
                    value_string(&mut arguments, "listen_value_missing")?,
                    "duplicate_listen",
                )?;
            }
            "--tls-cert" => {
                set_once(
                    &mut tls_cert,
                    value_path(&mut arguments, "tls_cert_value_missing")?,
                    "duplicate_tls_cert",
                )?;
            }
            "--tls-key" => {
                set_once(
                    &mut tls_key,
                    value_path(&mut arguments, "tls_key_value_missing")?,
                    "duplicate_tls_key",
                )?;
            }
            "--allow-non-loopback" => allow_non_loopback = true,
            "--public-origin" => {
                let value = value_string(&mut arguments, "public_origin_value_missing")?;
                validate_public_origin(&value)?;
                set_once(&mut public_origin, value, "duplicate_public_origin")?;
            }
            _ => return Err(ConfigError::Usage("unknown_argument")),
        }
    }
    let root = root.ok_or(ConfigError::Usage("root_required"))?;
    if command == CommandKind::Serve && (tls_cert.is_none() || tls_key.is_none()) {
        return Err(ConfigError::Usage("tls_required_for_serve"));
    }
    if command == CommandKind::AdminResetPassword {
        if admin_handle.as_deref().is_none_or(str::is_empty) {
            return Err(ConfigError::Usage("handle_required"));
        }
        if !password_stdin {
            return Err(ConfigError::Usage("password_stdin_required"));
        }
    }
    let listen = listen
        .or_else(|| {
            (command != CommandKind::AdminResetPassword)
                .then(|| env::var("SYLUX_LISTEN").ok())
                .flatten()
        })
        .unwrap_or_else(|| "127.0.0.1:3000".to_owned());
    let listen: SocketAddr = listen
        .parse()
        .map_err(|_| ConfigError::Usage("listen_invalid"))?;
    validate_bind_address(listen, allow_non_loopback)?;
    let assets = env::var_os("SYLUX_ASSETS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("web/dist"));
    Ok(CliConfig {
        command,
        root,
        listen,
        tls_cert,
        tls_key,
        allow_non_loopback,
        public_origin,
        assets,
        admin_handle,
    })
}

fn validate_public_origin(value: &str) -> Result<(), ConfigError> {
    let uri: axum::http::Uri = value
        .parse()
        .map_err(|_| ConfigError::Usage("public_origin_invalid"))?;
    let authority = uri
        .authority()
        .ok_or(ConfigError::Usage("public_origin_invalid"))?;
    if uri.scheme_str() != Some("https")
        || authority.host().is_empty()
        || authority.host().contains('*')
        || authority.as_str().contains('@')
        || value != format!("https://{authority}")
    {
        return Err(ConfigError::Usage("public_origin_invalid"));
    }
    Ok(())
}

fn value_string(
    arguments: &mut impl Iterator<Item = OsString>,
    code: &'static str,
) -> Result<String, ConfigError> {
    arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or(ConfigError::Usage(code))
}

fn value_path(
    arguments: &mut impl Iterator<Item = OsString>,
    code: &'static str,
) -> Result<PathBuf, ConfigError> {
    arguments
        .next()
        .map(PathBuf::from)
        .ok_or(ConfigError::Usage(code))
}

fn set_once<T>(slot: &mut Option<T>, value: T, code: &'static str) -> Result<(), ConfigError> {
    if slot.is_some() {
        return Err(ConfigError::Usage(code));
    }
    *slot = Some(value);
    Ok(())
}

/// PoC bind policy: loopback is always allowed; anything else requires the
/// explicit flag and must be a private (LAN/VLAN) address. Public, unspecified
/// and multicast addresses are always refused.
pub fn validate_bind_address(
    address: SocketAddr,
    allow_non_loopback: bool,
) -> Result<(), ConfigError> {
    if address.ip().is_loopback() {
        return Ok(());
    }
    if !allow_non_loopback {
        return Err(ConfigError::Usage("non_loopback_requires_flag"));
    }
    if is_private(address.ip()) {
        Ok(())
    } else {
        Err(ConfigError::Usage("address_not_private"))
    }
}

fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => v6.is_unique_local() || v6.is_unicast_link_local(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn args(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    #[test]
    fn recovery_parser_requires_explicit_complete_and_isolated_arguments() {
        let base = [
            "admin",
            "reset-password",
            "--root",
            "/x",
            "--handle",
            "owner",
            "--password-stdin",
        ];
        let parsed = parse(args(&base)).unwrap();
        assert_eq!(parsed.command, CommandKind::AdminResetPassword);
        assert_eq!(parsed.admin_handle.as_deref(), Some("owner"));
        for (input, reason) in [
            (vec!["admin"], "admin_subcommand_required"),
            (vec!["admin", "reset-password"], "root_required"),
            (
                vec!["admin", "reset-password", "--root", "/x"],
                "handle_required",
            ),
            (
                vec![
                    "admin",
                    "reset-password",
                    "--root",
                    "/x",
                    "--handle",
                    "owner",
                ],
                "password_stdin_required",
            ),
            (
                vec!["admin", "reset-password", "--root"],
                "root_value_missing",
            ),
            (
                vec!["admin", "reset-password", "--root", "/x", "--handle"],
                "handle_value_missing",
            ),
            (vec!["serve", "--handle", "owner"], "unknown_argument"),
            (vec!["init", "--password-stdin"], "unknown_argument"),
        ] {
            assert_eq!(parse(args(&input)).unwrap_err().reason(), reason);
        }
        for (extra, reason) in [
            (vec!["--root", "/y"], "duplicate_root"),
            (vec!["--handle", "other"], "duplicate_handle"),
            (vec!["--password-stdin"], "duplicate_password_stdin"),
            (vec!["--listen", "127.0.0.1:0"], "unknown_argument"),
            (vec!["--tls-cert", "x"], "unknown_argument"),
            (vec!["--tls-key", "x"], "unknown_argument"),
            (
                vec!["--public-origin", "https://example.test"],
                "unknown_argument",
            ),
            (vec!["--allow-non-loopback"], "unknown_argument"),
            (vec!["--password", "secret"], "unknown_argument"),
        ] {
            let mut input = base.to_vec();
            input.extend(extra);
            assert_eq!(parse(args(&input)).unwrap_err().reason(), reason);
        }
    }

    #[test]
    fn serve_requires_root_and_tls_pair() {
        assert_eq!(
            parse(args(&["serve", "--root", "/tmp/x"]))
                .err()
                .map(|e| e.reason()),
            Some("tls_required_for_serve")
        );
        assert_eq!(
            parse(args(&["serve"])).err().map(|e| e.reason()),
            Some("root_required")
        );
        assert_eq!(
            parse(args(&["init"])).err().map(|e| e.reason()),
            Some("root_required")
        );
    }

    #[test]
    fn parses_serve_and_init() {
        let config = parse(args(&[
            "serve",
            "--root",
            "/tmp/x",
            "--listen",
            "127.0.0.1:0",
            "--tls-cert",
            "a.pem",
            "--tls-key",
            "b.pem",
        ]))
        .unwrap();
        assert_eq!(config.command, CommandKind::Serve);
        assert_eq!(config.root, Path::new("/tmp/x"));
        assert_eq!(config.listen.port(), 0);
        assert!(config.public_origin.is_none());
        let config = parse(args(&[
            "init",
            "--root",
            "/tmp/x",
            "--listen",
            "127.0.0.1:0",
        ]))
        .unwrap();
        assert_eq!(config.command, CommandKind::Init);
        assert!(config.tls_cert.is_none());
    }

    #[test]
    fn rejects_unknown_and_duplicate_arguments() {
        assert_eq!(
            parse(args(&["serve", "--root", "/x", "--frobnicate"]))
                .err()
                .map(|e| e.reason()),
            Some("unknown_argument")
        );
        assert_eq!(
            parse(args(&["serve", "--root", "/x", "--root", "/y"]))
                .err()
                .map(|e| e.reason()),
            Some("duplicate_root")
        );
        assert_eq!(
            parse(args(&["wat"])).err().map(|e| e.reason()),
            Some("subcommand_required")
        );
    }

    #[test]
    fn public_origin_requires_absolute_https_origin_with_explicit_host() {
        let valid = parse(args(&[
            "serve",
            "--root",
            "/x",
            "--tls-cert",
            "a",
            "--tls-key",
            "b",
            "--public-origin",
            "https://sylux.example",
        ]))
        .unwrap();
        assert_eq!(
            valid.public_origin.as_deref(),
            Some("https://sylux.example")
        );
        for invalid in [
            "",
            "sylux.example",
            "http://sylux.example",
            "https://",
            "https://*",
            "https://sylux.example/path",
            "https://sylux.example/",
            "https://user@sylux.example",
        ] {
            assert_eq!(
                parse(args(&[
                    "serve",
                    "--root",
                    "/x",
                    "--tls-cert",
                    "a",
                    "--tls-key",
                    "b",
                    "--public-origin",
                    invalid,
                ]))
                .err()
                .map(|error| error.reason()),
                Some("public_origin_invalid"),
                "accepted invalid origin: {invalid:?}"
            );
        }
    }

    #[test]
    fn non_loopback_needs_flag_and_private_address() {
        let lan: SocketAddr = "192.168.1.10:3000".parse().unwrap();
        let public: SocketAddr = "8.8.8.8:3000".parse().unwrap();
        let any: SocketAddr = "0.0.0.0:3000".parse().unwrap();
        assert!(validate_bind_address("127.0.0.1:3000".parse().unwrap(), false).is_ok());
        assert_eq!(
            validate_bind_address(lan, false).err().map(|e| e.reason()),
            Some("non_loopback_requires_flag")
        );
        assert!(validate_bind_address(lan, true).is_ok());
        assert_eq!(
            validate_bind_address(public, true)
                .err()
                .map(|e| e.reason()),
            Some("address_not_private")
        );
        assert_eq!(
            validate_bind_address(any, true).err().map(|e| e.reason()),
            Some("address_not_private")
        );
        let ula: SocketAddr = "[fd00::1]:3000".parse().unwrap();
        assert!(validate_bind_address(ula, true).is_ok());
    }
}
