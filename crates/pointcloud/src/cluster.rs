//! Clustering a cloud: the classes the rules cannot give, named by the user.
//!
//! The rules of [`crate::classify`] explain a scan, but they cannot tell a chair from a bush: both
//! are a rough surface a metre above the ground. The way out is not a better rule but a *learned*
//! grouping: the points are described by what they are (the height above the ground, the
//! intensity of the return, the roughness of the surface, how much it stands up) and the points
//! that look alike are grouped together. The groups are then **named by the user** — the human in
//! the loop, which is how the practical classifiers are made.
//!
//! The grouping is the k-means of the features, with a fixed seed, so the same scan always gives
//! the same groups: a report can be repeated. Nothing is downloaded, no model is trained, no
//! weight file is shipped: the whole thing is a few hundred lines of arithmetic.
//!
//! The features are read on the sample of a level (the same sample the classifier uses) and the
//! group of a point is the group of its nearest sampled point.

use crate::classify::{CLASS_BUILDING, CLASS_GROUND, CLASS_HIGH_VEG, CLASS_LOW_VEG, CLASS_MED_VEG, CLASS_UNCLASSIFIED, Sample};

/// The features of a point, in the order they are used.
pub const FEATURES: usize = 4;

/// The height above the ground, in metres.
pub const FEATURE_HEIGHT: usize = 0;
/// The intensity of the return, between 0 and 1.
pub const FEATURE_INTENSITY: usize = 1;
/// The roughness of the surface, in metres.
pub const FEATURE_ROUGHNESS: usize = 2;
/// How much the surface stands up, between 0 and 1.
pub const FEATURE_VERTICALITY: usize = 3;

/// The name of a feature, the way the report writes it.
pub fn feature_name(index: usize) -> &'static str {
    match index {
        FEATURE_HEIGHT => "height",
        FEATURE_INTENSITY => "intensity",
        FEATURE_ROUGHNESS => "roughness",
        FEATURE_VERTICALITY => "verticality",
        _ => "other",
    }
}

/// What a group looks like.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cluster {
    /// The number of the group, from 0.
    pub index: usize,
    /// How many points are in it.
    pub count: usize,
    /// The middle of the group, in the features' own units: the height in metres, the intensity
    /// between 0 and 1, the roughness in metres, the verticality between 0 and 1.
    pub centre: [f64; FEATURES],
    /// The class the group was given.
    pub class: u8,
}

impl Cluster {
    /// The report line of a group.
    pub fn summary(&self) -> String {
        format!(
            "#{}: {} points — height {:.2} m, intensity {:.2}, roughness {:.3} m, verticality {:.2} — {}",
            self.index,
            self.count,
            self.centre[FEATURE_HEIGHT],
            self.centre[FEATURE_INTENSITY],
            self.centre[FEATURE_ROUGHNESS],
            self.centre[FEATURE_VERTICALITY],
            crate::classify::class_name(self.class)
        )
    }
}

/// The features of the sampled points.
pub fn features_of(sample: &Sample, intensities: &[u8]) -> Vec<[f64; FEATURES]> {
    let mut out = Vec::with_capacity(sample.len());
    for (n, &i) in sample.indices.iter().enumerate() {
        let intensity = intensities.get(i).copied().unwrap_or(0) as f64 / 255.0;
        out.push([f64::from(sample.heights[n]), intensity, f64::from(sample.shapes[n].roughness), f64::from(sample.shapes[n].verticality)]);
    }
    out
}

/// A small generator with a fixed seed, so the same scan always gives the same groups.
struct Rng(u64);

impl Rng {
    /// The next number, in 0..1.
    fn next(&mut self) -> f64 {
        // The constants of a 64-bit linear congruential generator.
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
    }
}

/// The groups of the features: the k-means, with a fixed seed.
///
/// The features are standardised first, so no feature dominates the others by its size alone; the
/// groups are found with the k-means++ seeding (the first group is a point at random, the next
/// the point that is farthest from the groups already found) and twenty rounds of refinement.
pub fn kmeans(features: &[[f64; FEATURES]], k: usize) -> (Vec<usize>, Vec<[f64; FEATURES]>) {
    let n = features.len();
    let k = k.clamp(1, 32).min(n.max(1));
    if n == 0 {
        return (Vec::new(), Vec::new());
    }
    // The standardisation: every feature has a mean of zero and a spread of one.
    let mut mean = [0.0f64; FEATURES];
    let mut spread = [0.0f64; FEATURES];
    for f in features {
        for (k2, v) in mean.iter_mut().enumerate() {
            *v += f[k2];
        }
    }
    for v in mean.iter_mut() {
        *v /= n as f64;
    }
    for f in features {
        for (k2, v) in spread.iter_mut().enumerate() {
            let d = f[k2] - mean[k2];
            *v += d * d;
        }
    }
    for v in spread.iter_mut() {
        *v = (*v / n as f64).sqrt();
        if !v.is_finite() || *v < 1e-9 {
            *v = 1.0;
        }
    }
    let scaled: Vec<[f64; FEATURES]> = features
        .iter()
        .map(|f| {
            let mut s = [0.0f64; FEATURES];
            for (k2, v) in s.iter_mut().enumerate() {
                *v = (f[k2] - mean[k2]) / spread[k2];
            }
            s
        })
        .collect();
    // The seeding: the first group at random, the next the farthest point.
    let mut rng = Rng(0x5EED_1234_ABCD_0001);
    let mut centres: Vec<[f64; FEATURES]> = Vec::with_capacity(k);
    centres.push(scaled[(rng.next() * n as f64) as usize % n]);
    let mut best = vec![f64::INFINITY; n];
    while centres.len() < k {
        let mut pick = 0usize;
        let mut farthest = -1.0f64;
        for (i, f) in scaled.iter().enumerate() {
            let d = distance(f, &centres[centres.len() - 1]);
            if d < best[i] {
                best[i] = d;
            }
            if best[i] > farthest {
                farthest = best[i];
                pick = i;
            }
        }
        if farthest <= 0.0 {
            // Every point is already in a group: the rest of the groups are copies.
            centres.push(centres[centres.len() - 1]);
        } else {
            centres.push(scaled[pick]);
        }
    }
    // The refinement.
    let mut labels = vec![0usize; n];
    for _ in 0..20 {
        let mut moved = false;
        for (i, f) in scaled.iter().enumerate() {
            let mut best_at = 0usize;
            let mut best_d = f64::INFINITY;
            for (c, centre) in centres.iter().enumerate() {
                let d = distance(f, centre);
                if d < best_d {
                    best_d = d;
                    best_at = c;
                }
            }
            if labels[i] != best_at {
                labels[i] = best_at;
                moved = true;
            }
        }
        // The new centres: the middle of their points.
        let mut sums = vec![[0.0f64; FEATURES]; k];
        let mut counts = vec![0usize; k];
        for (i, f) in scaled.iter().enumerate() {
            let c = labels[i];
            counts[c] += 1;
            for (k2, v) in sums[c].iter_mut().enumerate() {
                *v += f[k2];
            }
        }
        for c in 0..k {
            if counts[c] == 0 {
                continue;
            }
            for (k2, v) in centres[c].iter_mut().enumerate() {
                *v = sums[c][k2] / counts[c] as f64;
            }
        }
        if !moved {
            break;
        }
    }
    // The centres in the features' own units, so the report can be read.
    let mut centres: Vec<[f64; FEATURES]> = centres
        .into_iter()
        .map(|c| {
            let mut own = [0.0f64; FEATURES];
            for (k2, v) in own.iter_mut().enumerate() {
                *v = c[k2] * spread[k2] + mean[k2];
            }
            own
        })
        .collect();
    // The groups are ordered by their height, so the numbering means the same thing from one run
    // to the next: the lowest group is the first.
    let mut order: Vec<usize> = (0..centres.len()).collect();
    order.sort_by(|&a, &b| centres[a][FEATURE_HEIGHT].total_cmp(&centres[b][FEATURE_HEIGHT]));
    let mut new_index = vec![0usize; centres.len()];
    for (rank, &old) in order.iter().enumerate() {
        new_index[old] = rank;
    }
    for label in labels.iter_mut() {
        *label = new_index[*label];
    }
    let mut sorted: Vec<[f64; FEATURES]> = vec![[0.0; FEATURES]; centres.len()];
    for (old, centre) in centres.iter().enumerate() {
        sorted[new_index[old]] = *centre;
    }
    centres = sorted;
    (labels, centres)
}

/// The distance between two feature vectors.
fn distance(a: &[f64; FEATURES], b: &[f64; FEATURES]) -> f64 {
    let mut sum = 0.0;
    for k in 0..FEATURES {
        let d = a[k] - b[k];
        sum += d * d;
    }
    sum
}

/// The class a group gets by the rules, before the user names it.
///
/// The rules are simple and they are only a proposal: the lowest group is the ground, the
/// roughest is the vegetation, the one that stands up the most above the ground is a structure.
pub fn suggest(centres: &[[f64; FEATURES]], counts: &[usize]) -> Vec<u8> {
    let mut out = vec![CLASS_UNCLASSIFIED; centres.len()];
    if centres.is_empty() {
        return out;
    }
    // The lowest group: the ground.
    let mut lowest = 0usize;
    for (i, c) in centres.iter().enumerate() {
        if c[FEATURE_HEIGHT] < centres[lowest][FEATURE_HEIGHT] {
            lowest = i;
        }
    }
    out[lowest] = CLASS_GROUND;
    for (i, c) in centres.iter().enumerate() {
        if i == lowest {
            continue;
        }
        // A surface that stands up is a facade, a flat one high enough is a ceiling or a roof.
        let stands_up = c[FEATURE_VERTICALITY] > 0.65 && c[FEATURE_HEIGHT] > 0.5;
        let lies_flat = c[FEATURE_VERTICALITY] < 0.25 && c[FEATURE_HEIGHT] > 1.0;
        if stands_up || lies_flat {
            out[i] = CLASS_BUILDING;
        } else if c[FEATURE_HEIGHT] < 0.5 {
            out[i] = CLASS_LOW_VEG;
        } else if c[FEATURE_HEIGHT] < 2.0 {
            out[i] = CLASS_MED_VEG;
        } else {
            out[i] = CLASS_HIGH_VEG;
        }
    }
    let _ = counts;
    out
}

/// The groups of a level: their labels, their centres and their counts.
pub fn clusters_of(features: &[[f64; FEATURES]], k: usize) -> (Vec<usize>, Vec<Cluster>) {
    let (labels, centres) = kmeans(features, k);
    let mut counts = vec![0usize; centres.len()];
    for l in labels.iter() {
        counts[*l] += 1;
    }
    let classes = suggest(&centres, &counts);
    let clusters = centres.iter().enumerate().map(|(i, centre)| Cluster { index: i, count: counts[i], centre: *centre, class: classes[i] }).collect();
    (labels, clusters)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classify::{ClassifyOptions, Shape, sample_level};

    #[test]
    fn the_groups_are_the_same_from_one_run_to_the_next() {
        let features: Vec<[f64; FEATURES]> = (0..500).map(|i| if i < 250 { [0.2, 0.3, 0.01, 0.1] } else { [3.0, 0.7, 0.02, 0.9] }).collect();
        let (labels, centres) = kmeans(&features, 2);
        let (labels2, _) = kmeans(&features, 2);
        assert_eq!(labels, labels2, "the same scan gives the same groups");
        assert_eq!(centres.len(), 2);
        assert!(labels[..250].iter().all(|l| *l == 0), "the low points are together");
        assert!(labels[250..].iter().all(|l| *l == 1), "the high points are together");
    }

    #[test]
    fn the_groups_are_numbered_by_their_height() {
        let features: Vec<[f64; FEATURES]> = (0..300)
            .map(|i| {
                if i < 100 {
                    [3.0, 0.5, 0.02, 0.9]
                } else if i < 200 {
                    [0.2, 0.4, 0.01, 0.1]
                } else {
                    [1.0, 0.6, 0.03, 0.5]
                }
            })
            .collect();
        let (_, centres) = kmeans(&features, 3);
        assert!(centres[0][FEATURE_HEIGHT] < centres[1][FEATURE_HEIGHT]);
        assert!(centres[1][FEATURE_HEIGHT] < centres[2][FEATURE_HEIGHT]);
    }

    #[test]
    fn the_rules_propose_the_ground_the_vegetation_and_the_structure() {
        let centres = vec![[0.1, 0.3, 0.005, 0.05], [1.2, 0.5, 0.30, 0.45], [4.0, 0.6, 0.02, 0.95]];
        let classes = suggest(&centres, &[10, 10, 10]);
        assert_eq!(classes[0], CLASS_GROUND);
        assert_eq!(classes[1], CLASS_MED_VEG);
        assert_eq!(classes[2], CLASS_BUILDING);
    }

    #[test]
    fn the_features_of_a_sample_are_read() {
        // A floor with a wall above it.
        let mut points: Vec<[f64; 3]> = Vec::new();
        for i in 0..20 {
            for j in 0..20 {
                points.push([i as f64 * 0.1, j as f64 * 0.1, 0.0]);
            }
        }
        let ground = vec![CLASS_GROUND; points.len()];
        let options = ClassifyOptions { sample: 500, ..ClassifyOptions::default() };
        let sample = sample_level(&points, &ground, &options);
        assert!(!sample.is_empty());
        let intensities = vec![128u8; points.len()];
        let features = features_of(&sample, &intensities);
        assert_eq!(features.len(), sample.len());
        assert!(features[0][FEATURE_INTENSITY] > 0.4 && features[0][FEATURE_INTENSITY] < 0.6);
        let shape = Shape::default();
        assert_eq!(shape.neighbours, 0);
    }
}
