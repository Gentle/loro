use ctor::ctor;
use loro_core::container::Container;
use loro_core::LoroCore;

#[test]
fn test() {
    let mut store = LoroCore::new(Default::default(), None);
    let mut text_container = store.get_text_container("haha".into());
    text_container.insert(0, "abc");
    text_container.insert(1, "x");
    let value = text_container.get_value();
    let value = value.as_string().unwrap();
    assert_eq!(value.as_str(), "axbc");
}

#[ctor]
fn init_color_backtrace() {
    color_backtrace::install();
}
