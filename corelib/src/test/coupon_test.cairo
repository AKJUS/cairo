extern fn coupon_buy<T>() -> T nopanic;

#[feature("corelib-internal-use")]
fn arr_sum(arr: Array<(u128, arr_sum::Coupon)>) -> u128 nopanic {
    match arr.pop_front_consume() {
        Some((
            rem, (elm, coupon),
        )) => crate::integer::u128_wrapping_add(elm, arr_sum(rem, __coupon__: coupon)),
        None => 0,
    }
}

#[test]
fn test_arr_sum() {
    let mut arr: Array<(u128, arr_sum::Coupon)> = array![];
    arr.append((3, coupon_buy()));
    arr.append((4, coupon_buy()));
    arr.append((5, coupon_buy()));

    let available_gas = crate::testing::get_available_gas();
    let res = arr_sum(arr);
    // Check that arr_sum did not consume any gas.
    assert_ge!(core::testing::get_available_gas(), available_gas, "Gas was consumed by arr_sum.");
    assert_eq!(res, 12);
}
