use spinyarn::mapping::parse;

const FIXTURE_V1: &str = "tests/fixtures/test-mappings-v1.tiny";
const FIXTURE_V2: &str = "tests/fixtures/test-mappings.tiny";

#[test]
fn test_parse_v1_fixture() {
    let input = std::fs::read(FIXTURE_V1).unwrap();
    let m = parse(&input).expect("Failed to parse v1");

    assert_eq!(m.classes_len(), 2);
    assert_eq!(
        m.classes.get("class_1799").map(String::as_str),
        Some("net/minecraft/item/ItemStack")
    );
    assert_eq!(
        m.classes.get("class_1297").map(String::as_str),
        Some("net/minecraft/entity/Entity")
    );

    assert_eq!(m.methods_len(), 2);
    assert_eq!(m.methods.get("method_1234").map(String::as_str), Some("getCount"));
    assert_eq!(m.methods.get("method_6004").map(String::as_str), Some("tick"));

    assert_eq!(m.fields_len(), 2);
    assert_eq!(m.fields.get("field_8008").map(String::as_str), Some("size"));
}

#[test]
fn test_parse_v2_fixture() {
    let input = std::fs::read(FIXTURE_V2).unwrap();
    let m = parse(&input).expect("Failed to parse v2");

    assert_eq!(m.classes_len(), 2);
    assert_eq!(
        m.classes.get("class_1799").map(String::as_str),
        Some("net/minecraft/item/ItemStack")
    );
    assert_eq!(
        m.classes.get("class_1297").map(String::as_str),
        Some("net/minecraft/entity/Entity")
    );

    assert_eq!(m.methods.get("method_1234").map(String::as_str), Some("getCount"));
    assert_eq!(m.methods.get("method_6004").map(String::as_str), Some("tick"));

    assert_eq!(m.fields.get("field_8008").map(String::as_str), Some("size"));
}

#[test]
fn test_invalid_header() {
    let bad = b"bad_header\nc\ta\tb\n";
    let result = parse(bad);
    assert!(result.is_err());
}

#[test]
fn test_filters_non_obfuscated_keys() {
    // Readable official names (e.g. `run`, `a`) must be dropped from the tables.
    let v1 = b"v1\tofficial\tintermediary\tnamed\n\
               CLASS\ta\tnet/minecraft/class_1\tnet/minecraft/SomeClass\n\
               METHOD\ta\t()V\trun\trun\trun\n";
    let m = parse(v1).unwrap();
    assert_eq!(m.classes.get("class_1").map(String::as_str), Some("net/minecraft/SomeClass"));
    // `run` is not a method_ key -> excluded.
    assert!(m.methods.is_empty());
}
