use ldmud::loader::{Backend, Config};
use ldmud::value::*;

#[test]
fn interactive_flow() {
    let mut be = Backend::new(Config {
        mudlib_dir: "/opt/data/ldmud-rs/examples/mudlib".into(),
        master_file: "master.c".into(),
        port: 0,
    });
    be.boot_master().expect("boot");
    let oid = be.vm.handle_new_connection(1);
    println!("player oid={} interactive={}", oid, be.vm.env.interactive(oid));
    be.vm.receive_line(oid, "testplayer");
    let actions: Vec<String> = be.vm.env.actions.get(&oid).map(|a| a.iter().map(|x| x.verb.clone()).collect()).unwrap_or_default();
    println!("after name: actions={:?} enabled={:?}", actions, be.vm.env.commands_enabled.get(&oid));
    be.vm.receive_line(oid, "look");
    be.vm.receive_line(oid, "nosuch");
    for (slot, b) in &be.vm.out_q {
        println!("OUT slot{}: {:?}", slot, String::from_utf8_lossy(b));
    }
}
