use ldmud::parse::parse_file;

fn p(src: &str) -> Result<usize, String> {
    match parse_file("<t>", src) {
        Ok(c) => Ok(c.funcs.len()),
        Err(e) => Err(format!("{}:{}", e.line, e.msg)),
    }
}

#[test]
fn decls() {
    assert_eq!(p("string *files = ({ });\n").unwrap(), 0);
    assert_eq!(p("int add(int a, int b) { return a + b; }\n").unwrap(), 1);
}
