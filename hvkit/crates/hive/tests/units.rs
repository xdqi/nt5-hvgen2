use hive::reg::{self, value_line};
use hive::{REG_BINARY, REG_EXPAND_SZ, Value};

#[test]
fn hex_wraps_like_reg_exe() {
    // `"x"=hex:` is 8 columns, so 23 bytes fit on the first line (8 + 69 = 77, then the
    // backslash), then 25 per line.
    let v = Value::new("x", REG_BINARY, (0..60).collect());
    let lines: Vec<String> = value_line(&v).split("\r\n").map(String::from).collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0].len(), 78);
    assert!(lines[0].ends_with(",\\") && lines[1].starts_with("  ") && lines[1].len() == 78);
    assert!(lines[2].ends_with("3b"));
}

#[test]
fn strings_and_escapes() {
    assert_eq!(
        value_line(&Value::string("a\"b", hive::REG_SZ, "C:\\x")),
        r#""a\"b"="C:\\x""#
    );
    assert_eq!(
        value_line(&Value::string("", REG_EXPAND_SZ, "%a%")),
        "@=hex(2):25,00,61,00,25,00,00,00"
    );
    assert_eq!(
        value_line(&Value::dword("d", 0x1f)),
        r#""d"=dword:0000001f"#
    );
    // Unterminated or padded REG_SZ: the text up to the first NUL; odd length: hex(1).
    assert_eq!(
        value_line(&Value::new("s", hive::REG_SZ, vec![0x41, 0])),
        r#""s"="A""#
    );
    assert_eq!(
        value_line(&Value::new("s", hive::REG_SZ, vec![0x41, 0, 0, 0, 0x42, 0])),
        r#""s"="A""#
    );
    assert_eq!(
        value_line(&Value::new("s", hive::REG_SZ, vec![0x41, 0, 0])),
        r#""s"=hex(1):41,00,00"#
    );
}

#[test]
fn decode_both_encodings() {
    assert_eq!(reg::decode(&reg::to_utf16("abc")).unwrap(), "abc");
    assert_eq!(reg::decode(b"\xef\xbb\xbfabc").unwrap(), "abc");
}
