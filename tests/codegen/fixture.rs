#![allow(dead_code)]

use std::fmt;

fn value(input: u32) -> u32 {
    input + 1
}

fn large_frame() {
    let frame = [0u8; 4096];
    unsafe { std::ptr::read_volatile(&frame) };
}

fn formatting_frame(number: u32) {
    println!("{} {}", "literal", format_args!("{}", number));
}

struct Recursive;

impl fmt::Display for Recursive {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self)
    }
}

struct NestedRecursive;

impl fmt::Display for NestedRecursive {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", format_args!("{}", self))
    }
}

pub fn main() {
    assert!(!cfg!(clippy));
    assert_eq!(value(41), 42);
    large_frame();
    formatting_frame(7);
    thread_local! {
        static LOCAL: u32 = 1;
    }
    LOCAL.with(|value| assert_eq!(*value, 1));

    let original = String::from("owned");
    let _copy = original.clone();
    let number = 7;
    println!("{}", number);
    let _literal = format!("{}", "literal");
    println!("{}", &"literal");
    println!("{:>5}", format_args!("{}", number));
    println!("{}", format_args!("{}", number));
    let mut output = String::new();
    output.push_str(&format!("{number}"));
    assert_eq!(output, "7");
}
