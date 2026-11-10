//! Tests of `examples/common/icons.rs`: every icon is 48 x 48 and has ink.

use host_tests::icons::*;

#[test]
fn every_icon_has_ink_and_is_not_solid() {
    let all: [(&str, &[u8; 288]); 11] = [
        ("clear_sky", &CLEAR_SKY),
        ("fog", &FOG),
        ("gray", &GRAY),
        ("partly_cloudy", &PARTLY_CLOUDY),
        ("rain", &RAIN),
        ("snow", &SNOW),
        ("storm", &STORM),
        ("full_battery", &FULL_BATTERY),
        ("high_battery", &HIGH_BATTERY),
        ("half_battery", &HALF_BATTERY),
        ("low_battery", &LOW_BATTERY),
    ];
    for (name, bits) in all {
        let ink: u32 = bits.iter().map(|b| b.count_ones()).sum();
        assert!(ink > 40, "{name} has {ink} ink pixels");
        assert!(ink < 48 * 48 / 2, "{name} has {ink} ink pixels");
    }
}
