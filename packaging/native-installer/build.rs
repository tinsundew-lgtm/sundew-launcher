use std::{env,fs,path::PathBuf};
use sha2::{Digest,Sha256};
fn main() {
    println!("cargo:rerun-if-env-changed=LIMECRAFT_INSTALLER_PAYLOAD");
    let out=PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let source=env::var_os("LIMECRAFT_INSTALLER_PAYLOAD").map(PathBuf::from);
    let bytes=if let Some(p)=source { println!("cargo:rerun-if-changed={}",p.display()); let b=fs::read(&p).expect("Cannot read launcher payload"); assert!(b.starts_with(b"MZ"),"Payload must be Windows exe"); b } else {
        if env::var("PROFILE").as_deref()==Ok("release") { panic!("Set LIMECRAFT_INSTALLER_PAYLOAD to the release launcher exe"); }
        println!("cargo:warning=No payload: development UI only; installation disabled"); Vec::new()
    };
    fs::write(out.join("launcher.exe"),&bytes).unwrap();
    println!("cargo:rustc-env=LIMECRAFT_PAYLOAD_SHA256={}",hex::encode(Sha256::digest(&bytes)));
}
