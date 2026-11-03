//! Typed transparent draw ordering, shared by flat, textured, map and Marker paths.
use crate::spatial::marker_primitives::{Kind, Permit, TransparentInstance, UNKEYED};
use std::ops::Range;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Source {
    Vertices,
    Texture(usize),
    Map(usize),
    Marker(Kind),
}
impl Source {
    pub fn step(self) -> u32 {
        if matches!(self, Self::Marker(_)) {
            1
        } else {
            3
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub(super) struct Draw {
    pub source: Source,
    pub index: u32,
    pub depth: f32,
}
pub(super) fn sort(draws: &mut [Draw]) {
    draws.sort_unstable_by(|a, b| {
        b.depth
            .total_cmp(&a.depth)
            .then(a.source.cmp(&b.source))
            .then(a.index.cmp(&b.index))
    });
}
#[cfg(test)]
pub(super) fn run(draws: &[Draw], at: &mut usize) -> (Source, Range<u32>) {
    let first = draws[*at];
    let step = first.source.step();
    let mut end = first.index + step;
    *at += 1;
    while *at < draws.len() && draws[*at].source == first.source && draws[*at].index == end {
        end += step;
        *at += 1;
    }
    (first.source, first.index..end)
}
pub(super) struct Merge<'a> {
    standard: &'a [Draw],
    markers: &'a [TransparentInstance],
    i: usize,
    j: usize,
    pending: Option<Draw>,
    permit: Option<&'a Permit>,
}
#[cfg(test)]
pub(super) fn merge<'a>(standard: &'a [Draw], markers: &'a [TransparentInstance]) -> Merge<'a> {
    merge_visible(standard, markers, None)
}
pub(super) fn merge_visible<'a>(
    standard: &'a [Draw],
    markers: &'a [TransparentInstance],
    permit: Option<&'a Permit>,
) -> Merge<'a> {
    Merge {
        standard,
        permit,
        markers,
        i: 0,
        j: 0,
        pending: None,
    }
}
impl Merge<'_> {
    fn next_draw(&mut self) -> Option<Draw> {
        while self
            .markers
            .get(self.j)
            .is_some_and(|m| m.key != UNKEYED && !self.permit.is_some_and(|p| p.allows(m.key)))
        {
            self.j += 1;
        }
        let a = self.standard.get(self.i).copied();
        let b = self.markers.get(self.j).map(|m| Draw {
            source: Source::Marker(m.mesh),
            index: m.index,
            depth: m.depth,
        });
        match (a, b) {
            (Some(a), Some(b)) => {
                let first = b
                    .depth
                    .total_cmp(&a.depth)
                    .then(a.source.cmp(&b.source))
                    .then(a.index.cmp(&b.index));
                if first != std::cmp::Ordering::Greater {
                    self.i += 1;
                    Some(a)
                } else {
                    self.j += 1;
                    Some(b)
                }
            }
            (Some(a), None) => {
                self.i += 1;
                Some(a)
            }
            (None, Some(b)) => {
                self.j += 1;
                Some(b)
            }
            (None, None) => None,
        }
    }
}
impl Iterator for Merge<'_> {
    type Item = (Source, Range<u32>);
    fn next(&mut self) -> Option<Self::Item> {
        let first = self.pending.take().or_else(|| self.next_draw())?;
        let step = first.source.step();
        let mut end = first.index + step;
        while let Some(next) = self.next_draw() {
            if next.source == first.source && next.index == end {
                end += step;
            } else {
                self.pending = Some(next);
                break;
            }
        }
        Some((first.source, first.index..end))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn global_depth_order_and_ties_preserve_sources_and_merge_only_matching_contiguous_meshes() {
        let mut draws = vec![
            Draw {
                source: Source::Marker(Kind::Sphere),
                index: 1,
                depth: 0.2,
            },
            Draw {
                source: Source::Vertices,
                index: 0,
                depth: 0.8,
            },
            Draw {
                source: Source::Map(0),
                index: 0,
                depth: 0.5,
            },
            Draw {
                source: Source::Texture(0),
                index: 6,
                depth: 0.8,
            },
            Draw {
                source: Source::Marker(Kind::Cube),
                index: 0,
                depth: 0.2,
            },
            Draw {
                source: Source::Marker(Kind::Sphere),
                index: 2,
                depth: 0.2,
            },
        ];
        sort(&mut draws);
        assert_eq!(
            draws.iter().map(|d| d.source).collect::<Vec<_>>(),
            vec![
                Source::Vertices,
                Source::Texture(0),
                Source::Map(0),
                Source::Marker(Kind::Cube),
                Source::Marker(Kind::Sphere),
                Source::Marker(Kind::Sphere)
            ]
        );
        let mut at = 0;
        let mut runs = vec![];
        while at < draws.len() {
            runs.push(run(&draws, &mut at));
        }
        assert_eq!(
            runs,
            vec![
                (Source::Vertices, 0..3),
                (Source::Texture(0), 6..9),
                (Source::Map(0), 0..3),
                (Source::Marker(Kind::Cube), 0..1),
                (Source::Marker(Kind::Sphere), 1..3)
            ]
        );
    }
}

#[cfg(test)]
mod merge_tests {
    use super::*;
    #[test]
    fn sorted_marker_runs_split_at_other_scene_depths_without_copying_payloads() {
        let standard = [
            Draw {
                source: Source::Vertices,
                index: 0,
                depth: 0.6,
            },
            Draw {
                source: Source::Map(0),
                index: 0,
                depth: 0.4,
            },
        ];
        let markers = [
            TransparentInstance {
                key: UNKEYED,
                mesh: Kind::Cube,
                index: 0,
                depth: 0.9,
            },
            TransparentInstance {
                key: UNKEYED,
                mesh: Kind::Cube,
                index: 1,
                depth: 0.7,
            },
            TransparentInstance {
                key: UNKEYED,
                mesh: Kind::Cube,
                index: 2,
                depth: 0.5,
            },
            TransparentInstance {
                key: UNKEYED,
                mesh: Kind::Cylinder,
                index: 3,
                depth: 0.3,
            },
            TransparentInstance {
                key: UNKEYED,
                mesh: Kind::Cylinder,
                index: 4,
                depth: 0.2,
            },
        ];
        assert_eq!(
            merge(&standard, &markers).collect::<Vec<_>>(),
            vec![
                (Source::Marker(Kind::Cube), 0..2),
                (Source::Vertices, 0..3),
                (Source::Marker(Kind::Cube), 2..3),
                (Source::Map(0), 0..3),
                (Source::Marker(Kind::Cylinder), 3..5)
            ]
        );
        assert_eq!(merge(&[], &markers).count(), 2);
    }
    #[test]
    fn keyed_transparent_filter_cannot_coalesce_across_a_hidden_gap() {
        use crate::spatial::{
            marker_clock::MarkerClock,
            marker_geometry::Instance,
            marker_primitives::{Frame, KeyIdentity},
            CameraPose, RigidTransform,
        };
        let camera = CameraPose::top_down(8., [0.; 3]);
        let mut f = Frame::owned(MarkerClock::new());
        let instance = Instance {
            pose: RigidTransform::identity(),
            scale: [1.; 3],
            rgba: [1., 0., 0., 0.5],
        };
        for id in 0..3 {
            f.append_key(
                KeyIdentity {
                    owner: 1,
                    source_epoch: 1,
                    scene_epoch: 1,
                    asset_epoch: None,
                    key: studio_protocol::marker::Key {
                        namespace: "objects".into(),
                        id,
                    },
                    version: 1,
                },
                |f| f.push(Kind::Cube, &instance, camera, [128, 128]),
            )
            .unwrap();
        }
        let f = f.seal().unwrap();
        let permit = f.permit(|key| key.key.id != 1);
        assert_eq!(
            merge_visible(&[], f.transparent_instances(), Some(&permit)).collect::<Vec<_>>(),
            vec![
                (Source::Marker(Kind::Cube), 0..1),
                (Source::Marker(Kind::Cube), 2..3)
            ]
        );
        assert!(merge(&[], f.transparent_instances()).next().is_none());
    }
}
