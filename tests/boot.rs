use ldmud::loader::{Backend, Config};

#[test]
fn boot_and_load() {
    let mudlib = std::env::var("MUDLIB").unwrap_or_else(|_| "/opt/data/ldmud-rs/examples/mudlib".into());
    let cfg = Config { mudlib_dir: mudlib, master_file: "master.c".into(), port: 0 };
    let mut be = Backend::new(cfg);
    be.boot_master().expect("master boot");
    let oid = be.vm.load_object("/obj/player").expect("player load");
    println!("player oid = {}", oid);
    assert!(be.vm.has_lfun(oid, "logon"));
}
