//! Bounded URDF visual subset. Metadata is parsed for XML bounds, never evaluated.
use super::mesh::{Material, TextureReference};
use super::model::*;
use crate::spatial::RigidTransform;
use quick_xml::{events::Event, Reader};
use std::{
    collections::{BTreeMap, HashSet},
    sync::atomic::AtomicBool,
};

fn error(message: impl Into<String>) -> RobotError {
    RobotError::new(RobotErrorKind::InvalidModel, message)
}
fn unsupported(message: &str) -> RobotError {
    RobotError::new(RobotErrorKind::Unsupported, message)
}
#[derive(Debug)]
struct Node {
    tag: String,
    attrs: BTreeMap<String, String>,
    children: Vec<Node>,
    text: bool,
}
impl Node {
    fn attr(&self, name: &str) -> Result<&str, RobotError> {
        self.attrs
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| error(format!("{} requires {name}", self.tag)))
    }
    fn one(&self, tag: &str) -> Result<Option<&Node>, RobotError> {
        let mut found = self.children.iter().filter(|c| c.tag == tag);
        let first = found.next();
        if found.next().is_some() {
            return Err(error(format!("Repeated {tag} in {}", self.tag)));
        }
        Ok(first)
    }
    fn only(&self, tags: &[&str], attrs: &[&str]) -> Result<(), RobotError> {
        if self.text
            || self
                .children
                .iter()
                .any(|c| !tags.contains(&c.tag.as_str()))
            || self.attrs.keys().any(|a| !attrs.contains(&a.as_str()))
        {
            return Err(unsupported(
                "Unsupported URDF visual/model element, attribute or text",
            ));
        }
        Ok(())
    }
}
pub(super) enum Geometry {
    Ready(RobotGeometry),
    Mesh { filename: String, scale: [f64; 3] },
}
pub(super) struct Visual {
    pub role: GeometryRole,
    pub origin: RigidTransform,
    pub geometry: Geometry,
    pub rgba: [f32; 4],
    pub material_override: bool,
    pub texture: Option<TextureReference>,
}
pub(super) struct Link {
    pub name: String,
    pub visuals: Vec<Visual>,
}
pub(super) struct Description {
    pub name: String,
    pub links: Vec<Link>,
    pub joints: Vec<RobotJoint>,
    pub root_link: usize,
    pub material_count: usize,
}
fn name(value: &str) -> Result<String, RobotError> {
    if value.is_empty()
        || value.len() > MAX_NAME_BYTES
        || value.starts_with('/')
        || value.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(error("Invalid or overlong robot/link/joint/material name"));
    }
    Ok(value.into())
}
fn values<const N: usize>(source: &str) -> Result<[f64; N], RobotError> {
    let mut out = [0.0; N];
    let mut tokens = source.split_whitespace();
    for value in &mut out {
        *value = tokens
            .next()
            .and_then(|s| s.parse().ok())
            .filter(|v: &f64| v.is_finite())
            .ok_or_else(|| error("Expected finite numeric components"))?;
    }
    if tokens.next().is_some() {
        return Err(error("Unexpected extra numeric component"));
    }
    Ok(out)
}
fn positive(value: f64) -> Result<f64, RobotError> {
    if value > 0.0 && value <= MAX_DIMENSION {
        Ok(value)
    } else {
        Err(error(
            "Dimensions/scales must be positive and at most 10000",
        ))
    }
}
fn scalar(node: &Node, key: &str) -> Result<f64, RobotError> {
    positive(values::<1>(node.attr(key)?)?[0])
}
fn origin(node: Option<&Node>) -> Result<RigidTransform, RobotError> {
    let Some(node) = node else {
        return Ok(RigidTransform::identity());
    };
    node.only(&[], &["xyz", "rpy"])?;
    let xyz = node
        .attrs
        .get("xyz")
        .map(|s| values::<3>(s))
        .transpose()?
        .unwrap_or([0.; 3]);
    if xyz.iter().any(|v| v.abs() > MAX_COORDINATE) {
        return Err(error("Origin exceeds coordinate bounds"));
    }
    let [r, p, y] = node
        .attrs
        .get("rpy")
        .map(|s| values::<3>(s))
        .transpose()?
        .unwrap_or([0.; 3])
        .map(|v| v / 2.0);
    let (sr, cr) = r.sin_cos();
    let (sp, cp) = p.sin_cos();
    let (sy, cy) = y.sin_cos();
    RigidTransform::new(
        xyz,
        [
            sr * cp * cy - cr * sp * sy,
            cr * sp * cy + sr * cp * sy,
            cr * cp * sy - sr * sp * cy,
            cr * cp * cy + sr * sp * sy,
        ],
    )
    .ok_or_else(|| error("Invalid origin rotation"))
}
fn color(node: &Node) -> Result<Option<[f32; 4]>, RobotError> {
    node.only(&["color", "texture"], &["name"])?;
    let Some(c) = node.one("color")? else {
        return Ok(None);
    };
    c.only(&[], &["rgba"])?;
    let rgba = values::<4>(c.attr("rgba")?)?;
    if rgba.iter().any(|v| !(0.0..=1.0).contains(v)) {
        return Err(error("Material channels must lie in [0,1]"));
    }
    Ok(Some(rgba.map(|v| v as f32)))
}
fn definition(node: &Node) -> Result<Option<Material>, RobotError> {
    let rgba = color(node)?;
    let texture = node
        .one("texture")?
        .map(|t| {
            t.only(&[], &["filename"])?;
            let path = t.attr("filename")?;
            if path.is_empty() || path.len() > MAX_PATH_BYTES || path.chars().any(char::is_control)
            {
                return Err(error("Invalid texture filename"));
            }
            Ok(TextureReference {
                filename: path.into(),
                material_file: None,
                semantic: None,
            })
        })
        .transpose()?;
    if rgba.is_none() && texture.is_none() {
        return Ok(None);
    }
    Ok(Some(Material {
        rgba: rgba.unwrap_or([1.; 4]),
        texture,
    }))
}
fn material(
    node: Option<&Node>,
    named: &BTreeMap<String, Material>,
) -> Result<Material, RobotError> {
    let Some(node) = node else {
        return Ok(Material {
            rgba: [0.7, 0.7, 0.7, 1.0],
            texture: None,
        });
    };
    if let Some(n) = node.attrs.get("name") {
        name(n)?;
    }
    if let Some(value) = definition(node)? {
        return Ok(value);
    }
    let n = node.attr("name")?;
    named
        .get(n)
        .cloned()
        .ok_or_else(|| error(format!("Unknown material {n}")))
}
fn geometry(node: &Node) -> Result<Geometry, RobotError> {
    node.only(&["box", "cylinder", "sphere", "mesh"], &[])?;
    if node.children.len() != 1 {
        return Err(error("Visual geometry requires exactly one shape"));
    }
    let shape = &node.children[0];
    Ok(match shape.tag.as_str() {
        "box" => {
            shape.only(&[], &["size"])?;
            let size = values::<3>(shape.attr("size")?)?;
            for v in size {
                positive(v)?;
            }
            Geometry::Ready(RobotGeometry::Box { size })
        }
        "cylinder" => {
            shape.only(&[], &["radius", "length"])?;
            Geometry::Ready(RobotGeometry::Cylinder {
                radius: scalar(shape, "radius")?,
                length: scalar(shape, "length")?,
            })
        }
        "sphere" => {
            shape.only(&[], &["radius"])?;
            Geometry::Ready(RobotGeometry::Sphere {
                radius: scalar(shape, "radius")?,
            })
        }
        "mesh" => {
            shape.only(&[], &["filename", "scale"])?;
            let filename = shape.attr("filename")?.to_owned();
            if filename.is_empty() || filename.len() > MAX_PATH_BYTES {
                return Err(error("Invalid mesh path length"));
            }
            let scale = shape
                .attrs
                .get("scale")
                .map(|s| values::<3>(s))
                .transpose()?
                .unwrap_or([1.; 3]);
            for v in scale {
                positive(v)?;
            }
            Geometry::Mesh { filename, scale }
        }
        _ => return Err(unsupported("Unsupported visual geometry")),
    })
}
fn joint_metadata(node: &Node) -> Result<(), RobotError> {
    if let Some(axis) = node.one("axis")? {
        axis.only(&[], &["xyz"])?;
        let xyz = values::<3>(axis.attr("xyz")?)?;
        if xyz.iter().all(|v| *v == 0.0) {
            return Err(error("Joint axis must be nonzero"));
        }
    }
    for (tag, attrs) in [
        ("limit", &["lower", "upper", "effort", "velocity"][..]),
        ("dynamics", &["damping", "friction"][..]),
        (
            "safety_controller",
            &[
                "soft_lower_limit",
                "soft_upper_limit",
                "k_position",
                "k_velocity",
            ][..],
        ),
        ("calibration", &["rising", "falling"][..]),
        ("mimic", &["joint", "multiplier", "offset"][..]),
    ] {
        if let Some(metadata) = node.one(tag)? {
            metadata.only(&[], attrs)?;
            for (key, value) in &metadata.attrs {
                if key == "joint" {
                    name(value)?;
                } else {
                    let v = values::<1>(value)?[0];
                    if [
                        "effort",
                        "velocity",
                        "damping",
                        "friction",
                        "k_position",
                        "k_velocity",
                    ]
                    .contains(&key.as_str())
                        && v < 0.0
                    {
                        return Err(error("Negative joint metadata bound"));
                    }
                }
            }
            if tag == "mimic" {
                name(metadata.attr("joint")?)?;
            }
            if let (Some(lower), Some(upper)) =
                (metadata.attrs.get("lower"), metadata.attrs.get("upper"))
            {
                if values::<1>(lower)?[0] > values::<1>(upper)?[0] {
                    return Err(error("Joint lower bound exceeds upper bound"));
                }
            }
        }
    }
    Ok(())
}
fn xml_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}
fn tree(source: &str, cancel: &AtomicBool) -> Result<Node, RobotError> {
    if source.len() > MAX_URDF_BYTES {
        return Err(RobotError::new(
            RobotErrorKind::Limit,
            "URDF byte limit exceeded",
        ));
    }
    if source.contains('&') {
        return Err(unsupported("XML entities/references are unsupported"));
    }
    if source
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(error("Invalid XML control character"));
    }
    let mut reader = Reader::from_str(source);
    reader.config_mut().check_comments = true;
    let mut stack: Vec<Node> = Vec::new();
    let mut root = None;
    let mut elements = 0usize;
    let mut declaration = false;
    loop {
        super::stl::cancelled(cancel)?;
        let event = reader
            .read_event()
            .map_err(|e| RobotError::new(RobotErrorKind::Xml, e.to_string()))?;
        let empty = matches!(&event, Event::Empty(_));
        match event {
            Event::Start(e) | Event::Empty(e) => {
                elements += 1;
                if elements > MAX_XML_ELEMENTS || stack.len() + 1 > MAX_XML_DEPTH {
                    return Err(RobotError::new(
                        RobotErrorKind::Limit,
                        "XML depth/element budget exceeded",
                    ));
                }
                let tag = std::str::from_utf8(e.name().as_ref())
                    .map_err(|_| error("Non-UTF-8 XML name"))?
                    .to_owned();
                if tag.contains(':') || tag == "xacro" {
                    return Err(unsupported("XML namespaces/xacro are unsupported"));
                }
                if !xml_name(&tag) {
                    return Err(RobotError::new(RobotErrorKind::Xml, "Unsupported or malformed XML element name; URDF tags must use ASCII XML names"));
                }
                let mut attrs = BTreeMap::new();
                for a in e.attributes() {
                    let a = a.map_err(|e| error(e.to_string()))?;
                    let key = std::str::from_utf8(a.key.as_ref())
                        .map_err(|_| error("Invalid XML attribute"))?
                        .to_owned();
                    if key.contains(':') || key == "xmlns" {
                        return Err(unsupported("XML namespaces/xacro are unsupported"));
                    }
                    if !xml_name(&key) {
                        return Err(RobotError::new(
                            RobotErrorKind::Xml,
                            "Unsupported or malformed XML attribute name",
                        ));
                    }
                    let value = std::str::from_utf8(a.value.as_ref())
                        .map_err(|_| error("Non-UTF-8 XML attribute"))?
                        .to_owned();
                    if value.contains('<') {
                        return Err(error("Unescaped XML attribute content"));
                    }
                    if attrs.insert(key, value).is_some() {
                        return Err(error("Duplicate XML attribute"));
                    }
                }
                let node = Node {
                    tag,
                    attrs,
                    children: Vec::new(),
                    text: false,
                };
                // quick-xml's event variant is needed after owning element data.
                if empty {
                    attach(node, &mut stack, &mut root)?;
                } else {
                    stack.push(node);
                }
            }
            Event::End(e) => {
                let node = stack
                    .pop()
                    .ok_or_else(|| error("Unexpected XML closing tag"))?;
                if node.tag.as_bytes() != e.name().as_ref() {
                    return Err(error("Mismatched XML closing tag"));
                }
                attach(node, &mut stack, &mut root)?;
            }
            Event::Text(e) => {
                if e.as_ref().iter().any(|b| !b.is_ascii_whitespace()) {
                    if let Some(n) = stack.last_mut() {
                        n.text = true;
                    } else {
                        return Err(error("Text outside robot element"));
                    }
                }
            }
            Event::CData(_) => return Err(unsupported("XML CDATA is unsupported")),
            Event::DocType(_) | Event::GeneralRef(_) => {
                return Err(unsupported("DTD and entity references are unsupported"))
            }
            Event::PI(_) => return Err(unsupported("XML processing instructions are unsupported")),
            Event::Decl(e) => {
                if declaration || !stack.is_empty() || root.is_some() {
                    return Err(error("Misplaced XML declaration"));
                }
                declaration = true;
                if e.version().map_err(|e| error(e.to_string()))?.as_ref() != b"1.0" {
                    return Err(unsupported("Only XML 1.0 is supported"));
                }
                if let Some(encoding) = e.encoding() {
                    if !encoding
                        .map_err(|e| error(e.to_string()))?
                        .as_ref()
                        .eq_ignore_ascii_case(b"utf-8")
                    {
                        return Err(unsupported("Only UTF-8 URDF is supported"));
                    }
                }
            }
            Event::Comment(_) => {}
            Event::Eof => break,
        }
    }
    if !stack.is_empty() {
        return Err(error("Unclosed XML element"));
    }
    root.ok_or_else(|| error("Missing robot element"))
}
fn attach(node: Node, stack: &mut [Node], root: &mut Option<Node>) -> Result<(), RobotError> {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(node);
    } else if root.replace(node).is_some() {
        return Err(error("Multiple XML roots"));
    }
    Ok(())
}
pub(super) fn parse(source: &str, cancel: &AtomicBool) -> Result<Description, RobotError> {
    let root = tree(source, cancel)?;
    if root.tag != "robot" {
        return Err(error("Expected robot root element"));
    }
    root.only(
        &["link", "joint", "material", "transmission", "gazebo"],
        &["name", "version"],
    )?;
    let robot_name = name(root.attr("name")?)?;
    let mut named = BTreeMap::new();
    for node in root.children.iter().filter(|n| n.tag == "material") {
        let n = name(node.attr("name")?)?;
        let c =
            definition(node)?.ok_or_else(|| error("Named material requires color or texture"))?;
        if named.insert(n, c).is_some() {
            return Err(error("Duplicate named material"));
        }
        if named.len() > MAX_MATERIALS {
            return Err(RobotError::new(
                RobotErrorKind::Limit,
                "Named material limit exceeded",
            ));
        }
    }
    // Inline named definitions can be reused by other visuals, including earlier
    // references. Conflicting definitions never silently replace a material.
    for link in root.children.iter().filter(|n| n.tag == "link") {
        for visual in link.children.iter().filter(|n| n.tag == "visual") {
            if let Some(node) = visual.one("material")? {
                if let Some(n) = node.attrs.get("name") {
                    let n = name(n)?;
                    if let Some(c) = definition(node)? {
                        if named.get(&n).is_some_and(|previous| *previous != c) {
                            return Err(error("Conflicting named material definitions"));
                        }
                        named.insert(n, c);
                        if named.len() > MAX_MATERIALS {
                            return Err(RobotError::new(
                                RobotErrorKind::Limit,
                                "Named material limit exceeded",
                            ));
                        }
                    }
                }
            }
        }
    }
    let mut links = Vec::new();
    let mut names = BTreeMap::new();
    let mut visual_count = 0;
    for node in root.children.iter().filter(|n| n.tag == "link") {
        super::stl::cancelled(cancel)?;
        node.only(&["visual", "collision", "inertial"], &["name"])?;
        let n = name(node.attr("name")?)?;
        if names.insert(n.clone(), links.len()).is_some() {
            return Err(error("Duplicate link name"));
        }
        if links.len() >= MAX_LINKS {
            return Err(RobotError::new(
                RobotErrorKind::Limit,
                "Link count limit exceeded",
            ));
        }
        let mut visuals = Vec::new();
        for visual in node
            .children
            .iter()
            .filter(|n| n.tag == "visual" || n.tag == "collision")
        {
            let role = if visual.tag == "collision" {
                GeometryRole::Collision
            } else {
                GeometryRole::Visual
            };
            visual_count += 1;
            if visual_count > MAX_VISUALS {
                return Err(RobotError::new(
                    RobotErrorKind::Limit,
                    "Combined visual/collision count limit exceeded",
                ));
            }
            visual.only(
                if role == GeometryRole::Collision {
                    &["origin", "geometry"]
                } else {
                    &["origin", "geometry", "material"]
                },
                &["name"],
            )?;
            if let Some(n) = visual.attrs.get("name") {
                name(n)?;
            }
            let material = if role == GeometryRole::Collision {
                Material {
                    rgba: [0.9, 0.55, 0.15, 1.],
                    texture: None,
                }
            } else {
                material(visual.one("material")?, &named)?
            };
            visuals.push(Visual {
                role,
                origin: origin(visual.one("origin")?)?,
                geometry: geometry(
                    visual
                        .one("geometry")?
                        .ok_or_else(|| error("Missing visual/collision geometry"))?,
                )?,
                rgba: material.rgba,
                texture: material.texture,
                material_override: role == GeometryRole::Collision
                    || visual.one("material")?.is_some(),
            });
        }
        links.push(Link { name: n, visuals });
    }
    if links.is_empty() {
        return Err(error("Robot requires at least one link"));
    }
    let mut joints = Vec::new();
    let mut joint_names = HashSet::new();
    let mut parents = vec![None; links.len()];
    for node in root.children.iter().filter(|n| n.tag == "joint") {
        super::stl::cancelled(cancel)?;
        node.only(
            &[
                "origin",
                "parent",
                "child",
                "axis",
                "limit",
                "dynamics",
                "safety_controller",
                "calibration",
                "mimic",
            ],
            &["name", "type"],
        )?;
        let n = name(node.attr("name")?)?;
        if !joint_names.insert(n.clone()) {
            return Err(error("Duplicate joint name"));
        }
        if joints.len() >= MAX_JOINTS {
            return Err(RobotError::new(
                RobotErrorKind::Limit,
                "Joint count limit exceeded",
            ));
        }
        let kind = match node.attr("type")? {
            "fixed" => JointKind::Fixed,
            "revolute" => JointKind::Revolute,
            "continuous" => JointKind::Continuous,
            "prismatic" => JointKind::Prismatic,
            "floating" => JointKind::Floating,
            "planar" => JointKind::Planar,
            _ => return Err(unsupported("Unsupported joint type")),
        };
        joint_metadata(node)?;
        let resolve = |tag: &str| -> Result<usize, RobotError> {
            let n = node
                .one(tag)?
                .ok_or_else(|| error("Joint requires parent and child"))?;
            n.only(&[], &["link"])?;
            names
                .get(n.attr("link")?)
                .copied()
                .ok_or_else(|| error("Unknown joint link"))
        };
        let parent = resolve("parent")?;
        let child = resolve("child")?;
        if parent == child || parents[child].replace(parent).is_some() {
            return Err(error("Self joint or multiple parents"));
        }
        joints.push(RobotJoint {
            name: n,
            parent,
            child,
            kind,
            origin: origin(node.one("origin")?)?,
        });
    }
    for node in root.children.iter().filter(|n| n.tag == "joint") {
        if let Some(mimic) = node.one("mimic")? {
            if !joint_names.contains(mimic.attr("joint")?)
                || mimic.attr("joint")? == node.attr("name")?
            {
                return Err(error("Invalid mimic joint reference"));
            }
        }
    }
    let roots: Vec<_> = parents
        .iter()
        .enumerate()
        .filter(|(_, p)| p.is_none())
        .map(|(i, _)| i)
        .collect();
    if roots.len() != 1 || joints.len() + 1 != links.len() {
        return Err(error("Robot must form one connected tree"));
    }
    for start in 0..links.len() {
        let mut visited = HashSet::new();
        let mut at = start;
        while let Some(parent) = parents[at] {
            if !visited.insert(at) {
                return Err(error("Cyclic robot joint graph"));
            }
            at = parent;
        }
        if at != roots[0] {
            return Err(error("Disconnected robot graph"));
        }
    }
    let anonymous = root
        .children
        .iter()
        .filter(|n| n.tag == "link")
        .flat_map(|link| link.children.iter().filter(|n| n.tag == "visual"))
        .flat_map(|visual| visual.children.iter().filter(|n| n.tag == "material"))
        .filter(|material| !material.attrs.contains_key("name"))
        .count();
    let material_count = named
        .len()
        .checked_add(anonymous)
        .filter(|n| *n <= MAX_MATERIALS)
        .ok_or_else(|| {
            RobotError::new(
                RobotErrorKind::Limit,
                "Robot exceeds 128 material definitions",
            )
        })?;
    Ok(Description {
        name: robot_name,
        links,
        joints,
        root_link: roots[0],
        material_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn read(s: &str) -> Result<Description, RobotError> {
        parse(s, &AtomicBool::new(false))
    }
    #[test]
    fn forward_material_multiple_visuals_and_tree() {
        let d=read(r#"<robot name="r"><link name="base"><visual><geometry><box size="1 2 3"/></geometry><material name="red"/></visual><visual><geometry><sphere radius="1"/></geometry></visual></link><link name="tip"/><joint name="j" type="fixed"><parent link="base"/><child link="tip"/><origin xyz="1 0 0" rpy="0 0 1.5707963267948966"/></joint><material name="red"><color rgba="1 0 0 1"/></material></robot>"#).unwrap();
        assert_eq!(d.links[0].visuals.len(), 2);
        assert_eq!(d.links[0].visuals[0].rgba, [1., 0., 0., 1.]);
        assert_eq!(d.root_link, 0);
        assert!(
            (d.joints[0]
                .origin
                .apply(crate::spatial::Point3([1., 0., 0.]))
                .0[1]
                - 1.)
                .abs()
                < 1e-12
        );
    }
    #[test]
    fn rejects_unsafe_or_invalid_visual_models() {
        for xml in [
            r#"<!DOCTYPE robot><robot name="r"><link name="a"/></robot>"#,
            r#"<robot name="r"><link name="a"/><link name="b"/></robot>"#,
            r#"<robot name="r"><link name="a"><visual><geometry><sphere radius="NaN"/></geometry></visual></link></robot>"#,
            r#"<robot name="r"><link name="a"><visual><geometry><box size="1 1 1"/></geometry><material><color rgba="1 1 1 1.5"/></material></visual></link></robot>"#,
            r#"<robot name="r"><gazebo><xacro:thing/></gazebo><link name="a"/></robot>"#,
        ] {
            assert!(read(xml).is_err(), "{xml}");
        }
    }
}
