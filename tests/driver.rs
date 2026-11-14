//! End-to-end driver behaviour tests: run real LPC through the compiler,
//! VM, efuns and command machinery.

use ldmud::loader::{Backend, Config};
use ldmud::value::*;
use ldmud::vm::Ctrl;

fn vm() -> Backend {
    let mut be = Backend::new(Config {
        mudlib_dir: "/opt/data/ldmud-rs/examples/mudlib".into(),
        master_file: "master.c".into(),
        port: 0,
    });
    be.boot_master().expect("master boot");
    be
}

/// compile a snippet as an object and call __t() in it.
fn run(be: &mut Backend, body: &str) -> Result<Svalue, String> {
    let src = format!("mixed __t() {{ {} }}\n", body);
    let oid = be.vm.compile_string(&src)?;
    let r = be.vm.call_function(oid, "__t", &[]);
    be.vm.destruct(oid);
    match r {
        Ok(v) => Ok(v),
        Err(Ctrl::Error(e)) => Err(display_string(&e)),
        Err(e) => Err(format!("{:?}", e)),
    }
}

#[test]
fn arithmetic_and_strings() {
    let mut be = vm();
    assert_eq!(run(&mut be, "return 2 + 3 * 4;").unwrap(), Svalue::Int(14));
    assert_eq!(run(&mut be, "return (2 + 3) * 4;").unwrap(), Svalue::Int(20));
    assert_eq!(run(&mut be, "return 7 / 2;").unwrap(), Svalue::Int(3));
    assert_eq!(run(&mut be, "return 7 % 3;").unwrap(), Svalue::Int(1));
    assert_eq!(run(&mut be, "return 7.0 / 2;").unwrap(), Svalue::Float(3.5));
    assert_eq!(
        display_string(&run(&mut be, r#"return "foo" + "bar";"#).unwrap()),
        "foobar"
    );
    assert_eq!(
        display_string(&run(&mut be, r#"return sprintf("%d-%s-%O", 5, "x", ({1,2}));"#).unwrap()),
        "5-x-({ 1, 2 })"
    );
}

#[test]
fn control_flow() {
    let mut be = vm();
    assert_eq!(
        run(&mut be, "int s = 0; foreach (int x : ({1,2,3})) s += x; return s;").unwrap(),
        Svalue::Int(6)
    );
    assert_eq!(
        run(&mut be, "int s = 0; for (int i = 0; i < 3; i++) s += i; return s;").unwrap(),
        Svalue::Int(3)
    );
    assert_eq!(
        run(&mut be, "int s = 0; for (int i = 0, n = 3; i < n; i++) s += i; return s;").unwrap(),
        Svalue::Int(3)
    );
    assert_eq!(
        run(&mut be, "int i = 0; while (i < 5) { i++; if (i == 3) break; } return i;").unwrap(),
        Svalue::Int(3)
    );
    assert_eq!(
        run(
            &mut be,
            "switch (2) { case 1: return 10; case 2: return 20; default: return 30; }"
        )
        .unwrap(),
        Svalue::Int(20)
    );
    // range foreach
    assert_eq!(
        run(&mut be, "int s=0; foreach (int x : 1..4) s += x; return s;").unwrap(),
        Svalue::Int(10)
    );
}

#[test]
fn arrays_and_mappings() {
    let mut be = vm();
    assert_eq!(
        run(&mut be, "mixed a = ({1,2,3}); a += ({4}); return sizeof(a);").unwrap(),
        Svalue::Int(4)
    );
    assert_eq!(
        run(&mut be, r#"mixed m = ([ "a" : 1, "b" : 2 ]); m["c"] = 3; return m["a"] + m["b"] + m["c"];"#)
            .unwrap(),
        Svalue::Int(6)
    );
    assert_eq!(
        run(&mut be, r#"return implode(explode("a,b,c", ","), "-");"#).unwrap(),
        mk_string("a-b-c")
    );
    assert_eq!(
        run(&mut be, "return member_array(2, ({1,2,3}));").unwrap(),
        Svalue::Int(1)
    );
    // postfix on literals: array/mapping literals still take [idx], (call), ->
    assert_eq!(
        run(&mut be, "return ({1,2,3})[1];").unwrap(),
        Svalue::Int(2)
    );
    assert_eq!(
        run(&mut be, r#"return (["k":({7,8})])["k"][1];"#).unwrap(),
        Svalue::Int(8)
    );
    // mapping keys ordering preserved (%O gives LPC literal syntax)
    assert_eq!(
        display_string(&run(&mut be, r#"return sprintf("%O", keys((["z":1,"a":2])));"#).unwrap()),
        r#"({ "z", "a" })"#
    );
}

#[test]
fn closures_and_higher_order() {
    let mut be = vm();
    assert_eq!(
        display_string(
            &run(&mut be, r#"return sprintf("%O", map(({1,2,3}), (: $1 * 2 :)));"#).unwrap(),
        ),
        "({ 2, 4, 6 })"
    );
    assert_eq!(
        display_string(
            &run(&mut be, r#"return sprintf("%O", filter(({1,2,3,4}), (: $1 % 2 :)));"#).unwrap(),
        ),
        "({ 1, 3 })"
    );
    assert_eq!(
        run(&mut be, "mixed f = (: $1 + $2 :); return f(3, 4);").unwrap(),
        Svalue::Int(7)
    );
    // function-string lambda compilation
    assert_eq!(
        run(&mut be, r#"mixed f = bind_lambda(({$1}), "return $1 * 3;"); return f(5);"#).unwrap(),
        Svalue::Int(15)
    );
}

#[test]
fn sscanf_and_type_checks() {
    let mut be = vm();
    assert_eq!(
        display_string(
            &run(&mut be, r#"string a, b; int n; sscanf("x42y", "%s%d%s", a, n, b); return sprintf("%s/%d/%s", a, n, b);"#).unwrap()
        ),
        "x4/2/y" // LDMud %d: %s takes the largest match that still lets %d match
    );
    // catch/throw
    let r = run(&mut be, r#"catch(throw("boom")); write("after\n"); return 1;"#).unwrap();
    assert_eq!(r, Svalue::Int(1));
}

#[test]
fn call_other_and_objects() {
    let mut be = vm();
    let oid = be.vm.load_object("/obj/player").expect("player loads");
    assert!(be.vm.has_lfun(oid, "logon"));
    assert!(!be.vm.has_lfun(oid, "nope"));
    // master object exists and has connect
    assert!(be.vm.has_lfun(be.vm.env.master, "connect"));
    // object_name

    // a loaded object can be called from another context
    let r = be.vm.call_function(oid, "look_cmd", &[Svalue::Int(0), Svalue::Int(0)]);
    assert!(matches!(r, Ok(Svalue::Int(1))));}


#[test]
fn call_out_fires() {
    let mut be = vm();
    let src = r#"
    int fired = 0;
    mixed __t() { call_out("bump", 0); return 0; }
    void bump() { fired = 1; }
    mixed check() { return fired; }
    "#;
    let oid = be.vm.compile_string(src).unwrap();
    be.vm.call_function(oid, "__t", &[]).unwrap();
    be.vm.env.time_ms = be.vm.env.time_ms + 1000;
    be.vm.do_call_outs();
    let r = be.vm.call_function(oid, "check", &[]).unwrap();
    assert_eq!(r, Svalue::Int(1));
}

#[test]
fn variables_persist_across_calls() {
    let mut be = vm();
    let oid = be
        .vm
        .compile_string("int counter = 0; mixed inc() { counter++; return counter; }")
        .unwrap();
    assert_eq!(be.vm.call_function(oid, "inc", &[]).unwrap(), Svalue::Int(1));
    assert_eq!(be.vm.call_function(oid, "inc", &[]).unwrap(), Svalue::Int(2));
    // clone yields a distinct instance with independent state
    let c = be.vm.load_object_cloned("/obj/player").unwrap();
    assert_ne!(c, be.vm.load_object_cloned("/obj/player").unwrap());
}

#[test]
fn strings_primitives_catch_throw() {
    let mut be = vm();
    assert_eq!(
        display_string(&run(&mut be, r#"return capitalize("hello");"#).unwrap()),
        "Hello"
    );
    assert_eq!(
        display_string(&run(&mut be, r#"return sprintf("%d", strlen("abc") + sizeof(({1,2})));"#).unwrap()),
        "5"
    );
    // catch swallows a throw and execution continues
    assert_eq!(
        run(&mut be, r#"catch(throw("boom")); return 1;"#).unwrap(),
        Svalue::Int(1)
    );
}

#[test]
fn driver_hook_h_command() {
    let mut be = vm();
    let oid = be.vm.handle_new_connection(7);
    assert_ne!(oid, 0);
    be.vm.receive_line(oid, "hooker");
    let ob = be
        .vm
        .compile_string(
            r#"
        mixed install() { add_driver_hook(17, #'hook_handler); return 1; }
        int hook_handler(string str, object ob) {
            string what;
            if (sscanf(str, "hook %s", what) != 1) return 0;
            write("HOOK:" + what + "\n");
            return 1;
        }
        "#,
        )
        .unwrap();
    be.vm.call_function(ob, "install", &[]).unwrap();
    let before = be.vm.out_q.len();
    be.vm.receive_line(oid, "hook hello");
    let got: String = be.vm.out_q[before..]
        .iter()
        .map(|(_, b)| String::from_utf8_lossy(b).to_string())
        .collect();
    assert!(got.contains("HOOK:hello"), "got {:?}", got);
}

#[test]
fn interactive_login_commands_notify_fail() {
    let mut be = vm();
    let oid = be.vm.handle_new_connection(1);
    assert_ne!(oid, 0);
    be.vm.receive_line(oid, "alice"); // input_to("read_name")
    let after_login: String = be
        .vm
        .out_q
        .iter()
        .map(|(_, b)| String::from_utf8_lossy(b).to_string())
        .collect();
    assert!(after_login.contains("Hello, Alice"));
    assert!(after_login.contains("Alice> ")); // set_prompt closure fired
    // command dispatch
    let before = be.vm.out_q.len();
    be.vm.receive_line(oid, "look");
    let got: String = be.vm.out_q[before..]
        .iter()
        .map(|(_, b)| String::from_utf8_lossy(b).to_string())
        .collect();
    assert!(got.contains("bare room"));
    // verbs are reusable (LDMud does not drop actions after success)
    let before = be.vm.out_q.len();
    be.vm.receive_line(oid, "look");
    let got: String = be.vm.out_q[before..]
        .iter()
        .map(|(_, b)| String::from_utf8_lossy(b).to_string())
        .collect();
    assert!(got.contains("bare room"));
    // notify_fail
    let before = be.vm.out_q.len();
    be.vm.receive_line(oid, "echo");
    let got: String = be.vm.out_q[before..]
        .iter()
        .map(|(_, b)| String::from_utf8_lossy(b).to_string())
        .collect();
    assert!(got.contains("Echo what?"));
    // unknown verb
    let before = be.vm.out_q.len();
    be.vm.receive_line(oid, "frobnicate");
    let got: String = be.vm.out_q[before..]
        .iter()
        .map(|(_, b)| String::from_utf8_lossy(b).to_string())
        .collect();
    assert!(got.contains("Huh?"));
}
