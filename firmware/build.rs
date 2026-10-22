fn main() {
    println!("cargo::rerun-if-env-changed=ELD_UPLOAD_URL");
    println!("cargo::rerun-if-env-changed=ELD_UPLOAD_TOKEN");
    println!("cargo::rerun-if-env-changed=ELD_CELLULAR_UPLINK");
    println!("cargo::rustc-check-cfg=cfg(esp32)");
    println!("cargo::rustc-check-cfg=cfg(esp32s3)");
    println!("cargo::rustc-check-cfg=cfg(esp32s2)");
    println!("cargo::rustc-check-cfg=cfg(esp32p4)");
    embuild::espidf::sysenv::output();
}
