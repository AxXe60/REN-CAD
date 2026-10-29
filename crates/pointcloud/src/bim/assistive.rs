//! Assistive Scan-to-BIM (`A-Scan2BIM`: candidate wall enumeration, auto-regressive next-wall
//! prediction, corner/T-junction joining, and natural order metric).
//!
//! Inspired by Song et al., *"A-Scan2BIM: Assistive Scan to Building Information Modeling"*
//! (BMVC 2023). Rather than dumping an unstructured set of walls in arbitrary order, this stage:
//! 1. Enumerates candidate walls from the storey slice and measures their point-cloud support.
//! 2. Auto-regressively predicts the next wall(s) conditioned on the architect's modeling history
//!    (prioritising exterior perimeter loops, L-corner continuations from the last placed wall,
//!    orthogonal T-junctions, and room closures).
//! 3. Synthesises the corresponding BIM API operations (`WallCreate`, `WallJoin`, and
//!    `NewFamilyInstance` for openings) and evaluates the reconstruction order metric.

use crate::bim::openings::OpeningKind;
use crate::bim::storeys::{self, Storey};
use crate::bim::walls::{self, Wall};
use crate::bim::{BimError, BimOptions};

/// Classification of a candidate wall within the building hierarchy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateWallClass {
    /// Outer building envelope wall.
    Exterior,
    /// Full-height interior structural or room partition wall.
    Partition,
    /// Partial-height parapet or secondary core enclosure.
    Parapet,
}

/// A plausible wall extracted by Candidate Wall Enumeration, scored by the Next-Wall Predictor.
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateWall {
    /// Zero-based candidate identifier within the storey pool.
    pub id: usize,
    /// Underlying geometric wall definition.
    pub wall: Wall,
    /// Architectural hierarchy class.
    pub class: CandidateWallClass,
    /// Number of LiDAR points within the wall's support corridor.
    pub support_points: usize,
    /// Planar support confidence in `[0.0, 1.0]`.
    pub confidence: f64,
    /// Auto-regressive probability of being the next modeling operation in `[0.0, 1.0]`.
    pub next_probability: f64,
    /// 1-based rank among remaining unplaced candidates (`0` when already committed).
    pub rank: usize,
    /// Human-readable explanation of why the predictor ranked this wall next.
    pub rationale: String,
}

/// Type of topological junction between two connected walls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WallJoinKind {
    /// Two wall endpoints meet at a corner (`WallUtils.AllowWallJoinAtEnd`).
    LCorner,
    /// One wall endpoint butts into the interior span of another wall.
    TJunction,
}

/// One sequential BIM API editing operation in an A-Scan2BIM modeling session.
#[derive(Debug, Clone, PartialEq)]
pub enum AssistiveOperation {
    /// `Wall.Create(document, curve, levelId, structural)`
    WallCreate { candidate_id: usize, from: [f64; 2], to: [f64; 2], thickness: f64, height: f64 },
    /// `WallUtils.AllowWallJoinAtEnd(wall, end)`
    WallJoin { wall_a: usize, wall_b: usize, join_point: [f64; 2], kind: WallJoinKind },
    /// `Document.NewFamilyInstance(location, symbol, hostWall, level)`
    NewFamilyInstance { host_wall: usize, kind: OpeningKind, along: f64, width: f64, height: f64 },
}

fn dist_to_wall(pt: [f64; 2], w: &Wall) -> f64 {
    let dx = w.to[0] - w.from[0];
    let dy = w.to[1] - w.from[1];
    let len2 = dx * dx + dy * dy;
    if len2 <= 1e-12 {
        return (pt[0] - w.from[0]).hypot(pt[1] - w.from[1]);
    }
    let t = (((pt[0] - w.from[0]) * dx + (pt[1] - w.from[1]) * dy) / len2).clamp(0.0, 1.0);
    (pt[0] - (w.from[0] + t * dx)).hypot(pt[1] - (w.from[1] + t * dy))
}

/// Enumerates candidate walls in `storey` and scores their LiDAR point-cloud support.
///
/// # Errors
///
/// Propagates any [`BimError`] from [`walls::extract`].
pub fn enumerate_candidate_walls(points: &[[f64; 3]], storey: &Storey, opts: &BimOptions) -> Result<Vec<CandidateWall>, BimError> {
    let raw_walls = walls::extract(points, storey, opts)?;
    let storey_pts = storeys::points_in(points, storey, opts);

    let mut candidates = Vec::with_capacity(raw_walls.len());
    for (idx, w) in raw_walls.into_iter().enumerate() {
        let corridor = w.thickness * 0.5 + 0.12;
        let support_points = storey_pts
            .iter()
            .filter(|p| p[2] >= w.base_z - 0.05 && p[2] <= w.base_z + w.height + 0.05 && dist_to_wall([p[0], p[1]], &w) <= corridor)
            .count();

        let len = (w.to[0] - w.from[0]).hypot(w.to[1] - w.from[1]).max(0.4);
        let expected = (len * w.height.max(0.5) * 120.0).max(1.0);
        let ratio = (support_points as f64 / expected).clamp(0.0, 1.0);
        let confidence = 0.80 + 0.19 * ratio;

        let class = if w.is_parapet {
            CandidateWallClass::Parapet
        } else if w.exterior {
            CandidateWallClass::Exterior
        } else {
            CandidateWallClass::Partition
        };

        candidates.push(CandidateWall {
            id: idx,
            wall: w,
            class,
            support_points,
            confidence,
            next_probability: 0.0,
            rank: idx + 1,
            rationale: String::from("Enumerated from storey slice"),
        });
    }

    Ok(predict_next_walls(&candidates, &[]))
}

/// Auto-regressively scores and ranks unplaced candidate walls given the ordered `committed_ids`
/// modeling history.
#[must_use]
pub fn predict_next_walls(candidates: &[CandidateWall], committed_ids: &[usize]) -> Vec<CandidateWall> {
    let last_committed = committed_ids.last().and_then(|&id| candidates.iter().find(|c| c.id == id));

    let remaining_exterior = candidates.iter().filter(|c| !committed_ids.contains(&c.id) && c.class == CandidateWallClass::Exterior).count();

    let mut scored: Vec<CandidateWall> = candidates
        .iter()
        .map(|cand| {
            if let Some(pos) = committed_ids.iter().position(|&id| id == cand.id) {
                let mut done = cand.clone();
                done.next_probability = 1.0;
                done.rank = 0;
                done.rationale = format!("Committed at step #{}", pos + 1);
                return done;
            }

            let mut score = cand.confidence * 0.25;
            let mut rationale = String::from("High planar point-cloud support");

            // 1. Architectural phase prior (Exterior envelope -> Interior partitions -> Parapets)
            match cand.class {
                CandidateWallClass::Exterior => {
                    score += if remaining_exterior > 0 { 0.28 } else { 0.12 };
                    rationale = String::from("Exterior envelope perimeter wall");
                }
                CandidateWallClass::Partition => {
                    score += if remaining_exterior == 0 { 0.26 } else { 0.10 };
                    rationale = String::from("Interior structural partition wall");
                }
                CandidateWallClass::Parapet => {
                    score += if remaining_exterior == 0 { 0.14 } else { 0.03 };
                    rationale = String::from("Secondary parapet / partial-height wall");
                }
            }

            // 2. Auto-regressive continuation from the most recently placed wall w_t
            if let Some(last) = last_committed {
                let d11 = (cand.wall.from[0] - last.wall.to[0]).hypot(cand.wall.from[1] - last.wall.to[1]);
                let d12 = (cand.wall.to[0] - last.wall.to[0]).hypot(cand.wall.to[1] - last.wall.to[1]);
                let d21 = (cand.wall.from[0] - last.wall.from[0]).hypot(cand.wall.from[1] - last.wall.from[1]);
                let d22 = (cand.wall.to[0] - last.wall.from[0]).hypot(cand.wall.to[1] - last.wall.from[1]);
                let min_corner = d11.min(d12).min(d21).min(d22);

                if min_corner <= 0.35 {
                    score += 0.52;
                    rationale = format!("L-Corner continuation from Wall #{}", last.id + 1);
                } else {
                    let t_dist = dist_to_wall(cand.wall.from, &last.wall).min(dist_to_wall(cand.wall.to, &last.wall));
                    if t_dist <= 0.35 {
                        score += 0.36;
                        rationale = format!("Orthogonal T-junction with Wall #{}", last.id + 1);
                    }
                }
            } else {
                let len = (cand.wall.to[0] - cand.wall.from[0]).hypot(cand.wall.to[1] - cand.wall.from[1]);
                score += (len * 0.025).min(0.35);
                if cand.class == CandidateWallClass::Exterior {
                    rationale = String::from("Primary datum facade wall (Step #1)");
                }
            }

            // 3. Loop closure bonus with earlier walls in the modeling history
            if committed_ids.len() >= 2 {
                let mut touches = 0_usize;
                for &earlier_id in committed_ids.iter().take(committed_ids.len().saturating_sub(1)) {
                    if let Some(w) = candidates.iter().find(|c| c.id == earlier_id) {
                        let d = dist_to_wall(cand.wall.from, &w.wall).min(dist_to_wall(cand.wall.to, &w.wall));
                        if d <= 0.35 {
                            touches = touches.saturating_add(1);
                        }
                    }
                }
                if touches > 0 {
                    score += 0.18 * (touches.min(2) as f64);
                }
            }

            let mut next = cand.clone();
            next.next_probability = score.clamp(0.05, 0.99);
            next.rank = usize::MAX;
            next.rationale = rationale;
            next
        })
        .collect();

    // Sort unplaced candidate indices by descending score
    let mut unplaced_indices: Vec<usize> = scored.iter().enumerate().filter(|(_, c)| c.rank != 0).map(|(i, _)| i).collect();

    unplaced_indices.sort_by(|&a, &b| {
        let pa = scored.get(a).map_or(0.0, |c| c.next_probability);
        let pb = scored.get(b).map_or(0.0, |c| c.next_probability);
        pb.total_cmp(&pa)
    });

    let max_score = unplaced_indices.first().and_then(|&i| scored.get(i)).map_or(1.0, |c| c.next_probability.max(0.01));

    for (rank_zero, &vec_idx) in unplaced_indices.iter().enumerate() {
        if let Some(item) = scored.get_mut(vec_idx) {
            item.rank = rank_zero + 1;
            let calibrated = ((item.next_probability / max_score) * 0.96 - (rank_zero as f64) * 0.04).clamp(0.15, 0.98);
            item.next_probability = calibrated;
        }
    }

    scored
}

/// Rolls out the Next-Wall Predictor auto-regressively to produce a natural architectural sequence
/// of candidate wall IDs.
#[must_use]
pub fn plan_natural_sequence(candidates: &[CandidateWall]) -> Vec<usize> {
    let mut committed = Vec::with_capacity(candidates.len());
    for _ in 0..candidates.len() {
        let ranked = predict_next_walls(candidates, &committed);
        if let Some(top) = ranked.iter().find(|c| c.rank == 1) {
            committed.push(top.id);
        } else {
            break;
        }
    }
    committed
}

/// Synthesises the sequence of BIM API operations (`WallCreate` and `WallJoin`) for the given
/// ordered wall IDs.
#[must_use]
pub fn synthesize_bim_operations(candidates: &[CandidateWall], committed_ids: &[usize]) -> Vec<AssistiveOperation> {
    let mut ops = Vec::new();
    let mut placed: Vec<&CandidateWall> = Vec::new();

    for &cid in committed_ids {
        let Some(cand) = candidates.iter().find(|c| c.id == cid) else { continue };
        ops.push(AssistiveOperation::WallCreate {
            candidate_id: cand.id,
            from: cand.wall.from,
            to: cand.wall.to,
            thickness: cand.wall.thickness,
            height: cand.wall.height,
        });

        // Detect L-corner and T-junction joins with previously placed walls
        for prev in &placed {
            let endpoints = [
                (cand.wall.from, prev.wall.from),
                (cand.wall.from, prev.wall.to),
                (cand.wall.to, prev.wall.from),
                (cand.wall.to, prev.wall.to),
            ];
            let mut joined_corner = false;
            for (pa, pb) in endpoints {
                if (pa[0] - pb[0]).hypot(pa[1] - pb[1]) <= 0.35 {
                    ops.push(AssistiveOperation::WallJoin {
                        wall_a: prev.id,
                        wall_b: cand.id,
                        join_point: [(pa[0] + pb[0]) * 0.5, (pa[1] + pb[1]) * 0.5],
                        kind: WallJoinKind::LCorner,
                    });
                    joined_corner = true;
                    break;
                }
            }
            if !joined_corner {
                let d_from = dist_to_wall(cand.wall.from, &prev.wall);
                let d_to = dist_to_wall(cand.wall.to, &prev.wall);
                if d_from <= 0.35 {
                    let join_pt = cand.wall.from;
                    ops.push(AssistiveOperation::WallJoin { wall_a: prev.id, wall_b: cand.id, join_point: join_pt, kind: WallJoinKind::TJunction });
                } else if d_to <= 0.35 {
                    let join_pt = cand.wall.to;
                    ops.push(AssistiveOperation::WallJoin { wall_a: prev.id, wall_b: cand.id, join_point: join_pt, kind: WallJoinKind::TJunction });
                }
            }
        }
        placed.push(cand);
    }

    ops
}

/// Evaluates the **Order Metric** (normalised Kendall's $\tau$ concordance in `[0.0, 1.0]`)
/// between a `predicted_order` of wall IDs and a `reference_order` (Song et al., BMVC 2023).
#[must_use]
pub fn order_metric_kendall_tau(predicted_order: &[usize], reference_order: &[usize]) -> f64 {
    let common: Vec<usize> = predicted_order.iter().copied().filter(|id| reference_order.contains(id)).collect();
    let n = common.len();
    if n < 2 {
        return 1.0;
    }

    let mut concordant = 0_usize;
    let mut total_pairs = 0_usize;
    for i in 0..n {
        for j in (i + 1)..n {
            let (Some(&id_i), Some(&id_j)) = (common.get(i), common.get(j)) else { continue };
            let pos_i = reference_order.iter().position(|&r| r == id_i);
            let pos_j = reference_order.iter().position(|&r| r == id_j);
            if let (Some(pi), Some(pj)) = (pos_i, pos_j) {
                total_pairs = total_pairs.saturating_add(1);
                if pi < pj {
                    concordant = concordant.saturating_add(1);
                }
            }
        }
    }

    if total_pairs == 0 { 1.0 } else { concordant as f64 / total_pairs as f64 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_candidate(id: usize, from: [f64; 2], to: [f64; 2], class: CandidateWallClass) -> CandidateWall {
        CandidateWall {
            id,
            wall: Wall {
                from,
                to,
                thickness: 0.22,
                base_z: 0.0,
                height: 2.7,
                exterior: class == CandidateWallClass::Exterior,
                is_parapet: class == CandidateWallClass::Parapet,
                arc: None,
            },
            class,
            support_points: 1200,
            confidence: 0.94,
            next_probability: 0.0,
            rank: id + 1,
            rationale: String::new(),
        }
    }

    #[test]
    fn predictor_follows_connected_perimeter_loop_before_interior_partitions() {
        let cands = vec![
            sample_candidate(0, [0.0, 0.0], [8.0, 0.0], CandidateWallClass::Exterior),
            sample_candidate(1, [4.0, 0.0], [4.0, 6.0], CandidateWallClass::Partition),
            sample_candidate(2, [8.0, 0.0], [8.0, 6.0], CandidateWallClass::Exterior),
            sample_candidate(3, [8.0, 6.0], [0.0, 6.0], CandidateWallClass::Exterior),
            sample_candidate(4, [0.0, 6.0], [0.0, 0.0], CandidateWallClass::Exterior),
        ];

        let seq = plan_natural_sequence(&cands);
        assert_eq!(seq.len(), 5);
        // All 4 exterior perimeter walls come before the interior partition wall (id 1)
        assert_eq!(seq.last().copied(), Some(1));

        let ops = synthesize_bim_operations(&cands, &seq);
        let join_count = ops.iter().filter(|op| matches!(op, AssistiveOperation::WallJoin { .. })).count();
        assert!(join_count >= 4, "expected L-corner and T-junction joins");

        let tau = order_metric_kendall_tau(&seq, &[0, 2, 3, 4, 1]);
        assert!(tau >= 0.99);
    }
}
