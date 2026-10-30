//! Segment connectivity and closed contours of an editable vector network.
//!
//! A connected subpath follows shared vertices, including branches. A contour follows one closed
//! boundary returned by the face walk; nested boundaries are classified by containment. These
//! queries do not depend on whether a face has a fill.

use std::collections::VecDeque;

use figcraft_geom::kurbo::ParamCurve;
use figcraft_geom::{BezPath, Point, Shape};

use crate::VectorNetwork;

struct Contour {
    segments: Vec<usize>,
    path: BezPath,
    area: f64,
    sample: Point,
}

impl VectorNetwork {
    /// Every segment connected to `segment` through shared vertices. Branches remain part of the
    /// same subpath; disconnected rings remain separate.
    pub fn connected_subpath(&self, segment: usize) -> Vec<usize> {
        let Some(start) = self.segments.get(segment) else { return Vec::new() };
        if start.a >= self.vertices.len() || start.b >= self.vertices.len() {
            return Vec::new();
        }
        let mut incident = vec![Vec::new(); self.vertices.len()];
        for (index, edge) in self.segments.iter().enumerate() {
            if let Some(at) = incident.get_mut(edge.a) {
                at.push(index);
            }
            if edge.b != edge.a
                && let Some(at) = incident.get_mut(edge.b)
            {
                at.push(index);
            }
        }
        let mut seen = vec![false; self.segments.len()];
        let mut queue = VecDeque::from([segment]);
        seen[segment] = true;
        let mut selected = Vec::new();
        while let Some(index) = queue.pop_front() {
            let Some(edge) = self.segments.get(index) else { continue };
            selected.push(index);
            for vertex in [edge.a, edge.b] {
                if let Some(edges) = incident.get(vertex) {
                    for &next in edges {
                        if let Some(mark) = seen.get_mut(next)
                            && !*mark
                        {
                            *mark = true;
                            queue.push_back(next);
                        }
                    }
                }
            }
        }
        selected.sort_unstable();
        selected
    }

    /// The smallest closed contour containing `segment`. A segment at a junction can belong to
    /// more than one bounded face; the smaller boundary wins deterministically.
    pub fn closed_loop_at(&self, segment: usize) -> Option<Vec<usize>> {
        self.segments.get(segment)?;
        self.contours()
            .into_iter()
            .filter(|contour| contour.segments.contains(&segment))
            .min_by(|a, b| a.area.total_cmp(&b.area).then_with(|| a.segments.cmp(&b.segments)))
            .map(|contour| contour.segments)
    }

    /// Closed contours with no larger contour around them. Disconnected top-level contours are
    /// all returned; this is useful for vector networks containing several separate shapes.
    pub fn outer_contours(&self) -> Vec<Vec<usize>> {
        self.contours_by_nesting(false)
    }

    /// Closed contours nested inside at least one other contour, including islands inside holes.
    pub fn inner_contours(&self) -> Vec<Vec<usize>> {
        self.contours_by_nesting(true)
    }

    fn contours_by_nesting(&self, nested: bool) -> Vec<Vec<usize>> {
        let contours = self.contours();
        contours
            .iter()
            .filter(|contour| {
                let inside = contours.iter().any(|other| other.area > contour.area + 1e-9 && other.path.winding(contour.sample) != 0);
                inside == nested
            })
            .map(|contour| contour.segments.clone())
            .collect()
    }

    fn contours(&self) -> Vec<Contour> {
        let mut contours = Vec::new();
        for face in self.faces() {
            for mut segments in face.loops {
                let path = self.loop_path(&segments);
                let sample = segments.first().and_then(|index| self.cubic(*index)).map(|first| first.eval(0.5));
                segments.sort_unstable();
                segments.dedup();
                if contours.iter().any(|contour: &Contour| contour.segments == segments) {
                    continue;
                }
                let Some(sample) = sample else { continue };
                let area = path.area().abs();
                if !area.is_finite() || area <= 1e-9 {
                    continue;
                }
                contours.push(Contour { segments, path, area, sample });
            }
        }
        contours.sort_by(|a, b| a.segments.cmp(&b.segments));
        contours
    }
}

#[cfg(test)]
mod tests {
    use figcraft_geom::Point;

    use crate::{Segment, VectorNetwork, Vertex};

    fn square(net: &mut VectorNetwork, x: f64, y: f64, size: f64) {
        let first = net.vertices.len();
        let edge = net.segments.len();
        for (vx, vy) in [(x, y), (x + size, y), (x + size, y + size), (x, y + size)] {
            net.vertices.push(Vertex::new(vx, vy));
        }
        for i in 0..4 {
            net.segments.push(Segment::line(first + i, first + (i + 1) % 4));
        }
        assert_eq!(net.segments.len(), edge + 4);
    }

    #[test]
    fn nested_and_disconnected_contours_are_distinct() {
        let mut net = VectorNetwork::default();
        square(&mut net, 0.0, 0.0, 200.0);
        square(&mut net, 60.0, 60.0, 80.0);
        square(&mut net, 300.0, 0.0, 40.0);
        assert_eq!(net.connected_subpath(1), vec![0, 1, 2, 3]);
        assert_eq!(net.connected_subpath(5), vec![4, 5, 6, 7]);
        assert_eq!(net.closed_loop_at(5), Some(vec![4, 5, 6, 7]));
        assert_eq!(net.outer_contours(), vec![vec![0, 1, 2, 3], vec![8, 9, 10, 11]]);
        assert_eq!(net.inner_contours(), vec![vec![4, 5, 6, 7]]);
        assert!(net.closed_loop_at(99).is_none());
        assert!(net.connected_subpath(99).is_empty());
    }

    #[test]
    fn a_branch_connects_subpaths_but_is_not_a_closed_contour() {
        let mut net = VectorNetwork::default();
        square(&mut net, 0.0, 0.0, 100.0);
        net.vertices.push(Vertex::new(150.0, 0.0));
        net.segments.push(Segment::line(1, 4));
        assert_eq!(net.connected_subpath(4), vec![0, 1, 2, 3, 4]);
        assert_eq!(net.closed_loop_at(0), Some(vec![0, 1, 2, 3]));
        assert!(net.closed_loop_at(4).is_none());
        assert_eq!(net.outer_contours(), vec![vec![0, 1, 2, 3]]);
    }

    #[test]
    fn curved_contours_keep_their_nesting() {
        let mut net = VectorNetwork::default();
        let k = 0.552_284_749_830_793_6;
        for (center, radius) in [(Point::new(100.0, 100.0), 90.0), (Point::new(100.0, 100.0), 35.0)] {
            let start = net.vertices.len();
            for (x, y) in [(radius, 0.0), (0.0, radius), (-radius, 0.0), (0.0, -radius)] {
                net.vertices.push(Vertex::new(center.x + x, center.y + y));
            }
            for i in 0..4 {
                let a = start + i;
                let b = start + (i + 1) % 4;
                let mut edge = Segment::line(a, b);
                let (tx, ty) = match i {
                    0 => (0.0, 1.0),
                    1 => (-1.0, 0.0),
                    2 => (0.0, -1.0),
                    _ => (1.0, 0.0),
                };
                edge.ta = (tx * radius * k, ty * radius * k);
                edge.tb = (ty * radius * k, -tx * radius * k);
                net.segments.push(edge);
            }
        }
        assert_eq!(net.outer_contours(), vec![vec![0, 1, 2, 3]]);
        assert_eq!(net.inner_contours(), vec![vec![4, 5, 6, 7]]);
    }
}
