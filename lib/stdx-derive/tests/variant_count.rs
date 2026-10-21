#![allow(dead_code)]

use std::fmt::Debug;

use stdx_derive::VariantCount;

#[derive(VariantCount)]
enum Empty {}

#[derive(VariantCount)]
enum Single {
    A,
}

#[derive(VariantCount)]
enum Shapes {
    Unit,
    Tuple(u8, (u16, u32)),
    Struct { a: u8, b: Vec<u8> },
}

#[derive(VariantCount)]
#[repr(u8)]
enum Discriminants {
    A = 1 << 2,
    B = 16 >> 1,
    C = 1,
}

#[derive(VariantCount)]
enum Attributes {
    #[doc = "a, b"]
    A,
    #[allow(unused)]
    B { x: u8 },
}

#[derive(VariantCount)]
enum Defaults<A = u8, B = Vec<Option<u8>>> {
    A(A),
    B(B),
}

#[derive(VariantCount)]
enum Bounds<'a, T: Debug + 'a, F: Fn(u8) -> u8, const N: usize = 3> {
    A(&'a T),
    B([F; N]),
}

#[derive(VariantCount)]
enum WhereClause<T>
where
    T: Debug,
{
    A(T),
    B,
    C,
}

#[test]
fn counts_variants() {
    assert_eq!(Empty::VARIANT_COUNT, 0);
    assert_eq!(Single::VARIANT_COUNT, 1);
    assert_eq!(Shapes::VARIANT_COUNT, 3);
    assert_eq!(Discriminants::VARIANT_COUNT, 3);
    assert_eq!(Attributes::VARIANT_COUNT, 2);
}

#[test]
fn counts_variants_of_generic_enums() {
    assert_eq!(<Defaults>::VARIANT_COUNT, 2);
    assert_eq!(Defaults::<String, ()>::VARIANT_COUNT, 2);
    assert_eq!(Bounds::<u8, fn(u8) -> u8, 1>::VARIANT_COUNT, 2);
    assert_eq!(WhereClause::<u8>::VARIANT_COUNT, 3);
}
