//! IEC 61966-2-1's transfer function, re-exported from where it now lives.
//!
//! **It moved to [`narvo_render2d::srgb`] in M7.0.** The reason is the one this
//! crate's own header states in the opposite direction: fixture *data* lives
//! here, "**not the rules that check them**", and a transfer function fixed by a
//! published standard is the rule the `…UnormSrgb` formats obey rather than a
//! test helper. It had also become unreachable where it was — this crate is
//! `publish = false` and a dev-dependency (ADR-0016), so no production consumer
//! could call it, and two independent external consumers were measured writing
//! their own copy instead.
//!
//! This module is the re-export that keeps `narvo_testkit::srgb::…` resolving
//! for every caller that already wrote it, and `crate::srgb::…` resolving for
//! [`crate::text`]. That is what makes the move touch **no test file**: the
//! blessed scenes predicted through this path are an unmoved reference to moved
//! code. The same shape M6.6b used for [`crate::glyph_atlas`] and
//! [`crate::text`] (ADR-0038).

pub use narvo_render2d::srgb::{decode, encode};
