use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=RAVELINK_WDK_LIB");
    if env::var_os("CARGO_FEATURE_KERNEL").is_none()
        || env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
    {
        return;
    }
    let libraries = PathBuf::from(
        env::var_os("RAVELINK_WDK_LIB")
            .expect("RAVELINK_WDK_LIB must point to the pinned WDK km/x64 import libraries"),
    );
    for library in ["ntoskrnl", "hal", "portcls"] {
        assert!(
            libraries.join(format!("{library}.lib")).is_file(),
            "missing WDK import library {library}"
        );
        println!("cargo:rustc-link-lib={library}");
    }
    println!("cargo:rustc-link-search=native={}", libraries.display());
    for argument in [
        "/DRIVER",
        "/RELEASE",
        "/SUBSYSTEM:NATIVE",
        "/ENTRY:DriverEntry",
        "/NODEFAULTLIB",
        "/DYNAMICBASE",
        "/NXCOMPAT",
        "/INTEGRITYCHECK",
        "/OPT:REF",
        "/OPT:ICF",
    ] {
        println!("cargo:rustc-cdylib-link-arg={argument}");
    }
}
