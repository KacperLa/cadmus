//! Automatic frontlight calculations based on sunrise and sunset.
//!
//! The logic in this module keeps brightness and warmth aligned with the
//! current solar day for a given location.

use crate::geolocation::Coordinates;

use super::{LightLevel, LightLevels};
use chrono::{DateTime, NaiveDate, TimeDelta, Utc};

const WARMTH_TRANSITION_MINUTES: i64 = 90;

/// Covers adjacent UTC dates at extreme longitudes and neighboring high-latitude
/// solar days where a sunrise or sunset may be unavailable.
const SOLAR_BOUNDARY_SEARCH_DAYS: i64 = 2;

#[derive(Clone, Copy)]
struct SolarBoundary {
    time: DateTime<Utc>,
    event: sunrise::SolarEvent,
}

/// Computes the frontlight levels that should be active at the given time.
///
/// Brightness switches between `current_intensity` during daylight hours and
/// `night_brightness` while the sun is down. Warmth ramps over a fixed
/// transition window before sunrise and before sunset, reaching fully cool at
/// sunrise and fully warm at sunset.
///
/// When sunrise or sunset cannot be determined (polar regions), the function
/// falls back to constant levels: polar night (no sunrise) returns
/// `night_brightness` with full warmth, polar day (no sunset) returns
/// `current_intensity` with zero warmth.
pub fn compute_auto_frontlight_levels<Tz: chrono::TimeZone>(
    now: DateTime<Tz>,
    coordinates: Coordinates,
    night_brightness: LightLevel,
    current_intensity: LightLevel,
) -> LightLevels {
    let now = now.with_timezone(&Utc);
    let today = now.date_naive();
    let solar_coordinates: sunrise::Coordinates = coordinates.into();
    let solar_day = sunrise::SolarDay::new(solar_coordinates, today);
    let sunrise = solar_day.event_time(sunrise::SolarEvent::Sunrise);
    let sunset = solar_day.event_time(sunrise::SolarEvent::Sunset);

    if sunrise.is_none() {
        return light_levels(night_brightness, 1.0);
    }
    if sunset.is_none() {
        return light_levels(current_intensity, 0.0);
    }

    let (previous, next) = surrounding_solar_boundaries(now, solar_coordinates, today);
    let is_daytime = match (previous, next) {
        (Some(boundary), _) => boundary.event == sunrise::SolarEvent::Sunrise,
        (None, Some(boundary)) => boundary.event == sunrise::SolarEvent::Sunset,
        (None, None) => false,
    };
    let intensity = if is_daytime {
        current_intensity
    } else {
        night_brightness
    };
    let warmth_fraction = match (is_daytime, next) {
        (true, Some(boundary)) if boundary.event == sunrise::SolarEvent::Sunset => {
            transition_progress(now, boundary.time)
        }
        (false, Some(boundary)) if boundary.event == sunrise::SolarEvent::Sunrise => {
            1.0 - transition_progress(now, boundary.time)
        }
        (true, _) => 0.0,
        (false, _) => 1.0,
    };

    light_levels(intensity, warmth_fraction)
}

fn light_levels(intensity: LightLevel, warmth_fraction: f32) -> LightLevels {
    LightLevels {
        intensity,
        warmth: LightLevel::from_fraction(warmth_fraction),
    }
}

fn surrounding_solar_boundaries(
    now: DateTime<Utc>,
    coordinates: sunrise::Coordinates,
    center_date: NaiveDate,
) -> (Option<SolarBoundary>, Option<SolarBoundary>) {
    let boundaries = solar_boundaries(coordinates, center_date);
    (
        boundaries
            .iter()
            .rev()
            .find(|boundary| boundary.time <= now)
            .copied(),
        boundaries
            .iter()
            .find(|boundary| boundary.time > now)
            .copied(),
    )
}

fn solar_boundaries(
    coordinates: sunrise::Coordinates,
    center_date: NaiveDate,
) -> Vec<SolarBoundary> {
    let mut boundaries = Vec::with_capacity(10);

    for offset in -SOLAR_BOUNDARY_SEARCH_DAYS..=SOLAR_BOUNDARY_SEARCH_DAYS {
        let Some(date) = center_date.checked_add_signed(TimeDelta::days(offset)) else {
            continue;
        };
        let solar_day = sunrise::SolarDay::new(coordinates, date);

        for event in [sunrise::SolarEvent::Sunrise, sunrise::SolarEvent::Sunset] {
            if let Some(time) = solar_day.event_time(event) {
                boundaries.push(SolarBoundary { time, event });
            }
        }
    }

    boundaries.sort_by_key(|boundary| boundary.time);
    boundaries
}

fn transition_progress(now: DateTime<Utc>, boundary: DateTime<Utc>) -> f32 {
    let transition = TimeDelta::minutes(WARMTH_TRANSITION_MINUTES);
    let start = boundary - transition;

    ((now - start).num_seconds() as f32 / transition.num_seconds() as f32).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone};

    fn test_timezone() -> FixedOffset {
        FixedOffset::east_opt(60 * 60).unwrap()
    }

    fn wrapped_test_timezone() -> FixedOffset {
        FixedOffset::west_opt(4 * 60 * 60).unwrap()
    }

    fn london() -> Coordinates {
        Coordinates::new(51.5074, -0.1278).unwrap()
    }

    fn tromso() -> Coordinates {
        Coordinates::new(69.6492, 18.9553).unwrap()
    }

    fn make_dt(date: &str, time: &str) -> DateTime<FixedOffset> {
        let naive =
            chrono::NaiveDateTime::parse_from_str(&format!("{date} {time}"), "%Y-%m-%d %H:%M:%S")
                .unwrap();
        test_timezone()
            .from_local_datetime(&naive)
            .single()
            .unwrap()
    }

    fn solar_event(
        date: NaiveDate,
        coords: Coordinates,
        event: sunrise::SolarEvent,
        timezone: FixedOffset,
    ) -> DateTime<FixedOffset> {
        sunrise::SolarDay::new(coords.into(), date)
            .event_time(event)
            .expect("test location must have the solar event")
            .with_timezone(&timezone)
    }

    fn sunrise_at(
        date: NaiveDate,
        coords: Coordinates,
        timezone: FixedOffset,
    ) -> DateTime<FixedOffset> {
        solar_event(date, coords, sunrise::SolarEvent::Sunrise, timezone)
    }

    fn sunset_at(
        date: NaiveDate,
        coords: Coordinates,
        timezone: FixedOffset,
    ) -> DateTime<FixedOffset> {
        solar_event(date, coords, sunrise::SolarEvent::Sunset, timezone)
    }

    fn midpoint(start: DateTime<FixedOffset>, end: DateTime<FixedOffset>) -> DateTime<FixedOffset> {
        start + (end - start) / 2
    }

    fn summer_solstice() -> NaiveDate {
        NaiveDate::from_ymd_opt(2025, 6, 21).unwrap()
    }

    fn levels_at<Tz: chrono::TimeZone>(now: DateTime<Tz>, coordinates: Coordinates) -> LightLevels {
        compute_auto_frontlight_levels(now, coordinates, 10.0.into(), 50.0.into())
    }

    #[test]
    fn night_brightness_is_applied_when_sun_is_down() {
        let sunset = sunset_at(summer_solstice(), london(), test_timezone());
        let sunrise = sunrise_at(
            summer_solstice().succ_opt().unwrap(),
            london(),
            test_timezone(),
        );
        let nighttime = midpoint(sunset, sunrise);
        let levels = levels_at(nighttime, london());
        assert_eq!(
            levels.intensity, 10.0,
            "night brightness should be night_brightness"
        );
    }

    #[test]
    fn day_brightness_preserves_current_intensity() {
        let sunrise = sunrise_at(summer_solstice(), london(), test_timezone());
        let sunset = sunset_at(summer_solstice(), london(), test_timezone());
        let daytime = midpoint(sunrise, sunset);
        let levels = levels_at(daytime, london());
        assert_eq!(
            levels.intensity, 50.0,
            "day brightness should be current_intensity"
        );
    }

    #[test]
    fn wrapped_solar_day_uses_daytime_levels() {
        let timezone = wrapped_test_timezone();
        let sunrise = sunrise_at(summer_solstice(), london(), timezone);
        let sunset = sunset_at(summer_solstice(), london(), timezone);
        assert!(
            sunrise.time() > sunset.time(),
            "test setup must produce a wrapped solar day"
        );

        let daytime = midpoint(sunrise, sunset);
        let levels = levels_at(daytime, london());

        assert_eq!(levels.intensity, 50.0);
        assert!(levels.warmth < 1.0);
    }

    #[test]
    fn warmth_is_zero_at_sunrise() {
        let sunrise = sunrise_at(summer_solstice(), london(), test_timezone());
        let levels = levels_at(sunrise, london());
        assert!(
            levels.warmth < 1.0,
            "at sunrise warmth should be ~0, got {}",
            levels.warmth
        );
    }

    #[test]
    fn warmth_is_one_hundred_at_sunset() {
        let sunset = sunset_at(summer_solstice(), london(), test_timezone());
        let levels = levels_at(sunset, london());
        assert!(
            (levels.warmth - 100.0).abs() < 1.0,
            "at sunset warmth should be ~100, got {}",
            levels.warmth
        );
    }

    #[test]
    fn warmth_is_zero_during_middle_of_day() {
        let sunrise = sunrise_at(summer_solstice(), london(), test_timezone());
        let sunset = sunset_at(summer_solstice(), london(), test_timezone());
        let daytime = midpoint(sunrise, sunset);
        let levels = levels_at(daytime, london());
        assert!(
            levels.warmth < 1.0,
            "midday warmth should be ~0, got {}",
            levels.warmth
        );
    }

    #[test]
    fn warmth_is_one_hundred_during_middle_of_night() {
        let sunset = sunset_at(summer_solstice(), london(), test_timezone());
        let sunrise = sunrise_at(
            summer_solstice().succ_opt().unwrap(),
            london(),
            test_timezone(),
        );
        let nighttime = midpoint(sunset, sunrise);
        let levels = levels_at(nighttime, london());
        assert!(
            (levels.warmth - 100.0).abs() < 1.0,
            "midnight warmth should be ~100, got {}",
            levels.warmth
        );
    }

    #[test]
    fn warmth_ramps_from_zero_to_one_hundred_in_evening_transition() {
        let sunset = sunset_at(summer_solstice(), london(), test_timezone());
        let transition = TimeDelta::minutes(90);

        let t = sunset - transition;
        let levels = levels_at(t, london());
        assert!(
            levels.warmth < 2.0,
            "evening ramp start: warmth should be ~0, got {}",
            levels.warmth
        );

        let t = sunset - transition / 2;
        let levels = levels_at(t, london());
        assert!(
            (levels.warmth - 50.0).abs() < 6.0,
            "evening ramp midpoint: warmth should be ~50, got {}",
            levels.warmth
        );
    }

    #[test]
    fn warmth_ramps_from_one_hundred_to_zero_in_morning_transition() {
        let sunrise = sunrise_at(summer_solstice(), london(), test_timezone());
        let transition = TimeDelta::minutes(90);

        let t = sunrise - transition;
        let levels = levels_at(t, london());
        assert!(
            (levels.warmth - 100.0).abs() < 2.0,
            "morning ramp start: warmth should be ~100, got {}",
            levels.warmth
        );

        let t = sunrise - transition / 2;
        let levels = levels_at(t, london());
        assert!(
            (levels.warmth - 50.0).abs() < 6.0,
            "morning ramp midpoint: warmth should be ~50, got {}",
            levels.warmth
        );
    }

    #[test]
    fn evening_brightness_is_night_level() {
        let sunset = sunset_at(summer_solstice(), london(), test_timezone());
        let t = sunset + TimeDelta::minutes(30);
        let levels = levels_at(t, london());
        assert_eq!(
            levels.intensity, 10.0,
            "post-sunset brightness should be night_brightness"
        );
    }

    #[test]
    fn morning_brightness_is_night_level_before_sunrise() {
        let sunrise = sunrise_at(summer_solstice(), london(), test_timezone());
        let t = sunrise - TimeDelta::minutes(120);
        let levels = levels_at(t, london());
        assert_eq!(
            levels.intensity, 10.0,
            "pre-sunrise brightness should be night_brightness"
        );
    }

    #[test]
    fn same_instant_produces_same_levels_in_different_timezones() {
        let sunrise = sunrise_at(summer_solstice(), london(), test_timezone());
        let sunset = sunset_at(summer_solstice(), london(), test_timezone());
        let now = midpoint(sunrise, sunset);

        let local_levels = levels_at(now, london());
        let shifted_levels = levels_at(now.with_timezone(&wrapped_test_timezone()), london());

        assert_eq!(local_levels.intensity, shifted_levels.intensity);
        assert_eq!(local_levels.warmth, shifted_levels.warmth);
    }

    #[test]
    fn warmth_stays_continuous_across_midnight_when_morning_ramp_wraps() {
        let coordinates = tromso();
        let before_midnight = make_dt("2025-05-15", "23:59:00");
        let after_midnight = make_dt("2025-05-16", "00:00:00");

        let before_levels = levels_at(before_midnight, coordinates);
        let after_levels = levels_at(after_midnight, coordinates);

        assert!(
            (f32::from(before_levels.warmth) - f32::from(after_levels.warmth)).abs() < 4.0,
            "warmth should stay continuous across midnight, got {} then {}",
            before_levels.warmth,
            after_levels.warmth
        );
    }
}
