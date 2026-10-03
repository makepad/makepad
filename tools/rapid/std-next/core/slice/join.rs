//! `[S].concat()` / `[S].join(sep)` for strings and vectors (alloc's Concat/Join).

use crate::borrow::Borrow;
use crate::string::String;
use crate::vec::Vec;

pub trait Concat<Item: ?Sized> {
    type Output;
    fn concat(slice: &Self) -> Self::Output;
}

pub trait Join<Separator> {
    type Output;
    fn join(slice: &Self, sep: Separator) -> Self::Output;
}

pub fn concat_str<S: Borrow<str>>(slice: &[S]) -> String {
    join_str(slice, "")
}

pub fn join_str<S: Borrow<str>>(slice: &[S], sep: &str) -> String {
    let n = slice.len();
    if n == 0 {
        return String::new();
    }
    let mut total = sep.len() * (n - 1);
    let mut i = 0;
    while i < n {
        total += slice[i].borrow().len();
        i += 1;
    }
    let mut out = String::with_capacity(total);
    let mut i = 0;
    while i < n {
        if i > 0 {
            out.push_str(sep);
        }
        out.push_str(slice[i].borrow());
        i += 1;
    }
    out
}

pub fn concat_vec<T: Clone, V: Borrow<[T]>>(slice: &[V]) -> Vec<T> {
    let mut total = 0;
    let mut i = 0;
    while i < slice.len() {
        total += slice[i].borrow().len();
        i += 1;
    }
    let mut out = Vec::with_capacity(total);
    let mut i = 0;
    while i < slice.len() {
        out.extend_from_slice(slice[i].borrow());
        i += 1;
    }
    out
}

pub fn join_vec<T: Clone, V: Borrow<[T]>>(slice: &[V], sep: &[T]) -> Vec<T> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < slice.len() {
        if i > 0 {
            out.extend_from_slice(sep);
        }
        out.extend_from_slice(slice[i].borrow());
        i += 1;
    }
    out
}

impl<S: Borrow<str>> Concat<str> for [S] {
    type Output = String;
    fn concat(slice: &[S]) -> String {
        concat_str(slice)
    }
}

impl<'a, S: Borrow<str>> Join<&'a str> for [S] {
    type Output = String;
    fn join(slice: &[S], sep: &'a str) -> String {
        join_str(slice, sep)
    }
}

impl<T: Clone, V: Borrow<[T]>> Concat<T> for [V] {
    type Output = Vec<T>;
    fn concat(slice: &[V]) -> Vec<T> {
        concat_vec(slice)
    }
}

impl<'a, T: Clone, V: Borrow<[T]>> Join<&'a T> for [V] {
    type Output = Vec<T>;
    fn join(slice: &[V], sep: &'a T) -> Vec<T> {
        join_vec(slice, crate::slice::from_ref(sep))
    }
}

impl<'a, T: Clone, V: Borrow<[T]>> Join<&'a [T]> for [V] {
    type Output = Vec<T>;
    fn join(slice: &[V], sep: &'a [T]) -> Vec<T> {
        join_vec(slice, sep)
    }
}
