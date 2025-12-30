use std::env;
use std::path::Path;

#[cfg(all(feature = "loadable_extension", feature = "preupdate_hook"))]
compile_error!("feature \"loadable_extension\" and feature \"preupdate_hook\" cannot be enabled at the same time");

/// Tells whether we're building for Windows. This is more suitable than a plain
/// `cfg!(windows)`, since the latter does not properly handle cross-compilation
///
/// Note that there is no way to know at compile-time which system we'll be
/// targeting, and this test must be made at run-time (of the build script) See
/// <https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-build-scripts>
fn win_target() -> bool {
    env::var("CARGO_CFG_WINDOWS").is_ok()
}

/// Tells whether we're building for Android.
/// See [`win_target`]
#[cfg(any(feature = "bundled", feature = "bundled-windows"))]
fn android_target() -> bool {
    env::var("CARGO_CFG_TARGET_OS").is_ok_and(|v| v == "android")
}

/// Tells whether a given compiler will be used `compiler_name` is compared to
/// the content of `CARGO_CFG_TARGET_ENV` (and is always lowercase)
///
/// See [`win_target`]
fn is_compiler(compiler_name: &str) -> bool {
    env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|v| v == compiler_name)
}

/// Copy bindgen file from `dir` to `out_path`.
fn copy_bindings<T: AsRef<Path>>(dir: &str, bindgen_name: &str, out_path: T) {
    let from = if cfg!(feature = "loadable_extension") {
        format!("{dir}/{bindgen_name}_ext.rs")
    } else {
        format!("{dir}/{bindgen_name}.rs")
    };
    std::fs::copy(from, out_path).expect("Could not copy bindings to output directory");
}

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let out_path = Path::new(&out_dir).join("bindgen.rs");

    build_bundled::main(&out_dir, &out_path);
}

mod build_bundled {
    use std::env;
    use std::path::Path;

    #[expect(clippy::assertions_on_constants)] // https://github.com/rust-lang/rust-clippy/issues/16242
    pub fn main(out_dir: &str, out_path: &Path) {
        let devkitpro = env::var("DEVKITPRO").unwrap();
        let devkitarm = env::var("DEVKITARM").unwrap();
        println!("cargo:rerun-if-changed=build.rs");
        println!("cargo:rerun-if-env-changed=DEVKITPRO");
        println!("cargo:rustc-link-search=native={devkitpro}/libctru/lib");
        println!("cargo:rerun-if-changed={devkitpro}/libctru");

        let lib_name = super::lib_name();

        use super::{bindings, HeaderLocation};
        let header = HeaderLocation::FromPath(lib_name.to_owned());
        bindings::write_to_out_dir(header, out_path);
   
        println!("cargo:include={}/{lib_name}", env!("CARGO_MANIFEST_DIR"));
        println!("cargo:rerun-if-changed={lib_name}/sqlite3.c");
        println!("cargo:rerun-if-changed=sqlite3/wasm32-wasi-vfs.c");
        let mut cfg = cc::Build::new();

        let bin_dir = Path::new(devkitarm.as_str()).join("bin");
        let cc = bin_dir.join("arm-none-eabi-gcc");
        let ar = bin_dir.join("arm-none-eabi-ar");

        cfg.compiler(cc)
            .archiver(ar)
            .define("ARM11", None)
            .define("__3DS__", None)
            .flag("-march=armv6k")
            .flag("-mtune=mpcore")
            .flag("-mfloat-abi=hard")
            .flag("-mfpu=vfp")
            .flag("-mtp=soft")
            .flag("-Wno-deprecated-declarations")
            .host("arm-none-eabi");

        cfg.file(format!("{lib_name}/sqlite3.c"))
            .flag("-DSQLITE_CORE")
            .flag("-DSQLITE_OMIT_LOAD_EXTENSION")
            .flag("-DSQLITE_OMIT_DEPRECATED")
            .flag("-DSQLITE_OS_OTHER")
            .warnings(false);

        println!("cargo:rerun-if-env-changed=LIBSQLITE3_FLAGS");

        cfg.compile(lib_name);

        println!("cargo:lib_dir={out_dir}");
    }
}

fn env_prefix() -> &'static str {
    if cfg!(any(feature = "sqlcipher", feature = "bundled-sqlcipher")) {
        "SQLCIPHER"
    } else {
        "SQLITE3"
    }
}

fn lib_name() -> &'static str {
    if cfg!(any(feature = "sqlcipher", feature = "bundled-sqlcipher")) {
        "sqlcipher"
    } else {
        "sqlite3"
    }
}

pub enum HeaderLocation {
    FromEnvironment,
    Wrapper,
    FromPath(String),
}

impl From<HeaderLocation> for String {
    fn from(header: HeaderLocation) -> Self {
        match header {
            HeaderLocation::FromEnvironment => {
                let prefix = env_prefix();
                let mut header = env::var(format!("{prefix}_INCLUDE_DIR")).unwrap_or_else(|_| {
                    panic!("{prefix}_INCLUDE_DIR must be set if {prefix}_LIB_DIR is set")
                });
                header.push_str(if cfg!(feature = "loadable_extension") {
                    "/sqlite3ext.h"
                } else {
                    "/sqlite3.h"
                });
                header
            }
            HeaderLocation::Wrapper => if cfg!(feature = "loadable_extension") {
                "wrapper_ext.h"
            } else {
                "wrapper.h"
            }
            .into(),
            HeaderLocation::FromPath(path) => format!(
                "{}/{}",
                path,
                if cfg!(feature = "loadable_extension") {
                    "sqlite3ext.h"
                } else {
                    "sqlite3.h"
                }
            ),
        }
    }
}


#[cfg(not(feature = "buildtime_bindgen"))]
#[allow(dead_code)]
mod bindings {
    use super::HeaderLocation;

    use std::path::Path;

    static PREBUILT_BINDGENS: &[&str] = &["bindgen_3.34.1"];

    pub fn write_to_out_dir(_header: HeaderLocation, out_path: &Path) {
        let name = PREBUILT_BINDGENS[PREBUILT_BINDGENS.len() - 1];
        super::copy_bindings("bindgen-bindings", name, out_path);
    }
}

#[cfg(feature = "buildtime_bindgen")]
mod bindings {
    use super::HeaderLocation;
    use bindgen::callbacks::{IntKind, ParseCallbacks};

    use std::path::Path;
    #[derive(Debug)]
    struct SqliteTypeChooser;

    impl ParseCallbacks for SqliteTypeChooser {
        fn int_macro(&self, name: &str, _value: i64) -> Option<IntKind> {
            if name == "SQLITE_SERIALIZE_NOCOPY"
                || name.starts_with("SQLITE_DESERIALIZE_")
                || name.starts_with("SQLITE_PREPARE_")
                || name.starts_with("SQLITE_TRACE_")
            {
                Some(IntKind::UInt)
            } else {
                None
            }
        }
    }

    // Are we generating the bundled bindings? Used to avoid emitting things
    // that would be problematic in bundled builds. This env var is set by
    // `upgrade.sh`.
    fn generating_bundled_bindings() -> bool {
        // Hacky way to know if we're generating the bundled bindings
        println!("cargo:rerun-if-env-changed=LIBSQLITE3_SYS_BUNDLING");
        match std::env::var("LIBSQLITE3_SYS_BUNDLING") {
            Ok(v) => v != "0",
            Err(_) => false,
        }
    }

    pub fn write_to_out_dir(header: HeaderLocation, out_path: &Path) {
        let header: String = header.into();
        let mut bindings = bindgen::builder()
            .default_macro_constant_type(bindgen::MacroTypeVariation::Signed)
            .disable_nested_struct_naming()
            .generate_cstr(true)
            .use_core()
            .trust_clang_mangling(false)
            .header(header.clone())
            .parse_callbacks(Box::new(SqliteTypeChooser));
        if cfg!(feature = "loadable_extension") {
            bindings = bindings.ignore_functions(); // see generate_functions
        } else {
            bindings = bindings
                .blocklist_function("sqlite3_auto_extension")
                .raw_line(
                    r#"extern "C" {
    pub fn sqlite3_auto_extension(
        xEntryPoint: ::core::option::Option<
            unsafe extern "C" fn(
                db: *mut sqlite3,
                pzErrMsg: *mut *mut ::core::ffi::c_char,
                _: *const sqlite3_api_routines,
            ) -> ::core::ffi::c_int,
        >,
    ) -> ::core::ffi::c_int;
}"#,
                )
                .blocklist_function("sqlite3_cancel_auto_extension")
                .raw_line(
                    r#"extern "C" {
    pub fn sqlite3_cancel_auto_extension(
        xEntryPoint: ::core::option::Option<
            unsafe extern "C" fn(
                db: *mut sqlite3,
                pzErrMsg: *mut *mut ::core::ffi::c_char,
                _: *const sqlite3_api_routines,
            ) -> ::core::ffi::c_int,
        >,
    ) -> ::core::ffi::c_int;
}"#,
                )
                .blocklist_function(".*16.*")
                .blocklist_function("sqlite3_close_v2")
                .blocklist_function("sqlite3_create_collation")
                .blocklist_function("sqlite3_create_function")
                .blocklist_function("sqlite3_create_module")
                .blocklist_function("sqlite3_prepare");
        }

        if cfg!(any(feature = "sqlcipher", feature = "bundled-sqlcipher")) {
            bindings = bindings.clang_arg("-DSQLITE_HAS_CODEC");
        }
        if cfg!(feature = "unlock_notify") {
            bindings = bindings.clang_arg("-DSQLITE_ENABLE_UNLOCK_NOTIFY");
        }
        if cfg!(feature = "preupdate_hook") {
            bindings = bindings.clang_arg("-DSQLITE_ENABLE_PREUPDATE_HOOK");
        }
        if cfg!(feature = "session") {
            bindings = bindings.clang_arg("-DSQLITE_ENABLE_SESSION");
        }

        // When cross compiling unless effort is taken to fix the issue, bindgen
        // will find the wrong headers. There's only one header included by the
        // amalgamated `sqlite.h`: `stdarg.h`.
        //
        // Thankfully, there's almost no case where rust code needs to use
        // functions taking `va_list` (It's nearly impossible to get a `va_list`
        // in Rust unless you get passed it by C code for some reason).
        //
        // Arguably, we should never be including these, but we include them for
        // the cases where they aren't totally broken...
        let target_arch = std::env::var("TARGET").unwrap();
        let host_arch = std::env::var("HOST").unwrap();
        let is_cross_compiling = target_arch != host_arch;

        // Note that when generating the bundled file, we're essentially always
        // cross compiling.
        if generating_bundled_bindings() || is_cross_compiling {
            // Get rid of va_list, as it's not
            bindings = bindings
                .blocklist_function("sqlite3_vmprintf")
                .blocklist_function("sqlite3_vsnprintf")
                .blocklist_function("sqlite3_str_vappendf")
                .blocklist_type("va_list")
                .blocklist_item("__.*");
        }

        let bindings = bindings
            .layout_tests(false)
            .generate()
            .unwrap_or_else(|_| panic!("could not run bindgen on header {header}"));

        #[cfg(feature = "loadable_extension")]
        {
            let mut output = Vec::new();
            bindings
                .write(Box::new(&mut output))
                .expect("could not write output of bindgen");
            let mut output = String::from_utf8(output).expect("bindgen output was not UTF-8?!");
            super::loadable_extension::generate_functions(&mut output);
            std::fs::write(out_path, output.as_bytes())
                .unwrap_or_else(|_| panic!("Could not write to {out_path:?}"));
        }
        #[cfg(not(feature = "loadable_extension"))]
        bindings
            .write_to_file(out_path)
            .unwrap_or_else(|_| panic!("Could not write to {out_path:?}"));
    }
}

