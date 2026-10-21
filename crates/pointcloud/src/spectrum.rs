//! The colour scales an intensity band can be read on.
//!
//! Every scale is drawn here, in code: no picture, no font, no third-party palette. The names are
//! the ones the survey practice uses for the print and the screen — the grey of the report, the
//! thermal blue-to-red, the perceptually even viridis, the rainbow turbo, the magma of the relief
//! and the diverging scale of the comparison.
//!
//! A scale is a list of stops, from the low return to the high one, and the value between two
//! stops is mixed, so the band reads as a continuous ramp.

/// A colour scale for the intensity (or the elevation) of a cloud.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spectrum {
    /// Black to white: what a printed report shows.
    Grey,
    /// Dark blue, cyan, green, yellow, red: the thermal scale of the screen.
    BlueRed,
    /// Dark violet to yellow: even steps of light, so a weak difference is still a difference.
    Viridis,
    /// The rainbow, but ordered so it does not repeat itself.
    Turbo,
    /// Black, violet, orange, pale yellow: the scale of a relief.
    Magma,
    /// Blue, green, yellow, red: the diverging scale of the comparison.
    Diverging,
}

impl Spectrum {
    /// Every scale, in the order the palette lists them.
    pub const ALL: [Spectrum; 6] = [Spectrum::Grey, Spectrum::BlueRed, Spectrum::Viridis, Spectrum::Turbo, Spectrum::Magma, Spectrum::Diverging];

    /// The stops of the scale, from the low value to the high one.
    pub fn stops(self) -> &'static [[f32; 3]] {
        match self {
            Spectrum::Grey => &[[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]],
            Spectrum::BlueRed => &[[0.16, 0.20, 0.55], [0.15, 0.60, 0.85], [0.20, 0.80, 0.45], [0.95, 0.85, 0.25], [0.90, 0.20, 0.15]],
            Spectrum::Viridis => &[[0.27, 0.00, 0.33], [0.23, 0.32, 0.55], [0.13, 0.57, 0.55], [0.37, 0.79, 0.38], [0.99, 0.91, 0.14]],
            Spectrum::Turbo => {
                &[[0.19, 0.07, 0.23], [0.13, 0.45, 0.85], [0.10, 0.85, 0.79], [0.55, 0.95, 0.20], [0.95, 0.79, 0.16], [0.87, 0.20, 0.10]]
            }
            Spectrum::Magma => {
                &[[0.00, 0.00, 0.02], [0.23, 0.06, 0.35], [0.55, 0.16, 0.42], [0.87, 0.35, 0.33], [0.99, 0.71, 0.38], [0.99, 0.99, 0.75]]
            }
            Spectrum::Diverging => &[[0.15, 0.30, 0.85], [0.15, 0.70, 0.85], [0.20, 0.80, 0.35], [0.95, 0.85, 0.20], [0.90, 0.20, 0.15]],
        }
    }

    /// The colour of a value, `t` between 0 and 1.
    pub fn apply(self, t: f32) -> [u8; 3] {
        let stops = self.stops();
        let x = t.clamp(0.0, 1.0) * (stops.len() - 1) as f32;
        let i = (x.floor() as usize).min(stops.len() - 2);
        let f = x - i as f32;
        let (a, b) = (stops[i], stops[i + 1]);
        let mix = |k: usize| ((a[k] * (1.0 - f) + b[k] * f) * 255.0).round().clamp(0.0, 255.0) as u8;
        [mix(0), mix(1), mix(2)]
    }

    /// The name of the scale, the way the command and the palette write it.
    pub fn name(self) -> &'static str {
        match self {
            Spectrum::Grey => "grey",
            Spectrum::BlueRed => "bluered",
            Spectrum::Viridis => "viridis",
            Spectrum::Turbo => "turbo",
            Spectrum::Magma => "magma",
            Spectrum::Diverging => "diverging",
        }
    }

    /// The name a person would write: the same word, with a space or a dash where it reads better.
    pub fn label(self) -> &'static str {
        match self {
            Spectrum::Grey => "Grey",
            Spectrum::BlueRed => "Blue to red",
            Spectrum::Viridis => "Viridis",
            Spectrum::Turbo => "Turbo",
            Spectrum::Magma => "Magma",
            Spectrum::Diverging => "Diverging",
        }
    }

    /// The scale a name stands for.
    pub fn parse(text: &str) -> Option<Self> {
        let key: String = text.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        match key.as_str() {
            "grey" | "gray" | "graycale" | "greyscale" | "monochrome" => Some(Spectrum::Grey),
            "bluered" | "thermal" | "rainbow" | "jet" | "classic" => Some(Spectrum::BlueRed),
            "viridis" => Some(Spectrum::Viridis),
            "turbo" => Some(Spectrum::Turbo),
            "magma" | "inferno" | "plasma" => Some(Spectrum::Magma),
            "diverging" | "deviation" | "compare" => Some(Spectrum::Diverging),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scale_runs_from_its_low_stop_to_its_high_one() {
        for s in Spectrum::ALL {
            let low = s.apply(0.0);
            let high = s.apply(1.0);
            let stops = s.stops();
            let first = stops[0];
            let last = stops[stops.len() - 1];
            for k in 0..3 {
                assert_eq!(low[k], (first[k] * 255.0).round() as u8, "{s:?}");
                assert_eq!(high[k], (last[k] * 255.0).round() as u8, "{s:?}");
            }
        }
    }

    #[test]
    fn the_grey_scale_reads_black_to_white() {
        assert_eq!(Spectrum::Grey.apply(0.0), [0, 0, 0]);
        assert_eq!(Spectrum::Grey.apply(1.0), [255, 255, 255]);
        let mid = Spectrum::Grey.apply(0.5);
        assert_eq!(mid, [128, 128, 128]);
    }

    #[test]
    fn a_scale_outside_the_range_keeps_its_ends() {
        assert_eq!(Spectrum::Grey.apply(-1.0), [0, 0, 0]);
        assert_eq!(Spectrum::Grey.apply(2.0), [255, 255, 255]);
    }

    #[test]
    fn the_names_are_read_the_way_they_are_written() {
        assert_eq!(Spectrum::parse("Blue-Red"), Some(Spectrum::BlueRed));
        assert_eq!(Spectrum::parse("viridis"), Some(Spectrum::Viridis));
        assert_eq!(Spectrum::parse("Gray"), Some(Spectrum::Grey));
        assert_eq!(Spectrum::parse("plasma"), Some(Spectrum::Magma));
        assert_eq!(Spectrum::parse("nope"), None);
    }

    #[test]
    fn the_names_are_unique() {
        let mut names: Vec<&str> = Spectrum::ALL.iter().map(|s| s.name()).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count);
    }
}
