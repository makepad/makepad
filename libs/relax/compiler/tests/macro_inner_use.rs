//! Inner module attributes export the same textual scope as outer attributes.
#[path = "macro_inner_use/definitions.rs"]
mod definitions;
#[path = "macro_inner_use/consumer.rs"]
mod consumer;

mod inline {
    #![cfg_attr(all(), macro_use)]
    macro_rules! inline_value { () => { 23u32 }; }
}

macro_rules! confined_value { () => { 3u32 }; }
mod boundary {
    mod child {
        #![macro_use]
        macro_rules! confined_value { () => { 4u32 }; }
    }
    pub fn value() -> u32 { confined_value!() }
}

macro_rules! configured_value { () => { 5u32 }; }
mod disabled {
    #![cfg_attr(any(), macro_use)]
    macro_rules! configured_value { () => { 6u32 }; }
    pub fn value() -> u32 { configured_value!() }
}

#[test]
fn inner_attributes_preserve_module_boundaries_and_configuration() {
    assert_eq!(consumer::value(), 17);
    assert_eq!(file_value!(), 17);
    assert_eq!(inline_value!(), 23);
    assert_eq!(boundary::value(), 4);
    assert_eq!(confined_value!(), 3);
    assert_eq!(disabled::value(), 6);
    assert_eq!(configured_value!(), 5);
}
