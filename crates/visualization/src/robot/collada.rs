//! Static, local COLLADA subset with bounds applied before allocation/instancing.
//! Mesh axes follow RViz; declared document units scale geometry and translations.
mod xml;
use super::{
    mesh::{self, Decoded, Material, Part, TextureReference},
    stl::cancelled,
    RobotError, RobotErrorKind, Triangle, TriangleUv, MAX_COORDINATE, MAX_DIMENSION, MAX_MATERIALS,
    MAX_NAME_BYTES, MAX_ROBOT_TRIANGLES, MAX_VISUALS,
};
use std::{collections::BTreeMap, sync::atomic::AtomicBool};
use xml::Node;
const MAX_FLOATS: usize = 1_200_000;
const MAX_ARRAY: usize = 450_000;
const MAX_RECORDS: usize = 150_000;
const MAX_INDICES: usize = 1_200_000;
const MAX_SOURCES: usize = 512;
const MAX_GEOMETRIES: usize = 128;
const MAX_NODE_VISITS: usize = 1024;
fn bad(s: &str) -> RobotError {
    RobotError::new(RobotErrorKind::Asset, format!("Collada: {s}"))
}
fn limit(s: &str) -> RobotError {
    RobotError::new(RobotErrorKind::Limit, format!("Collada: {s}"))
}
fn unsupported(s: &str) -> RobotError {
    RobotError::new(RobotErrorKind::Unsupported, format!("Collada: {s}"))
}
fn id(s: &str) -> Result<(), RobotError> {
    if s.is_empty()
        || s.len() > MAX_NAME_BYTES
        || s.bytes()
            .any(|b| !b.is_ascii_alphanumeric() && !matches!(b, b'_' | b'-' | b'.'))
    {
        return Err(bad("invalid or overlong id/reference"));
    }
    Ok(())
}
fn reference(s: &str) -> Result<&str, RobotError> {
    let s = s
        .strip_prefix('#')
        .ok_or_else(|| unsupported("only local #id references are supported"))?;
    id(s)?;
    Ok(s)
}
fn integer(s: &str, max: usize) -> Result<usize, RobotError> {
    let n = s
        .parse::<usize>()
        .map_err(|_| bad("invalid nonnegative integer"))?;
    if n > max {
        return Err(limit("declared count/stride exceeds limit"));
    }
    Ok(n)
}
fn number(s: &str) -> Result<f64, RobotError> {
    s.parse::<f64>()
        .ok()
        .filter(|n| n.is_finite() && n.abs() <= MAX_COORDINATE)
        .ok_or_else(|| bad("nonfinite or out-of-range number"))
}
fn fixed<const N: usize>(s: &str) -> Result<[f64; N], RobotError> {
    let mut w = s.split_whitespace();
    let mut a = [0.; N];
    for n in &mut a {
        *n = number(w.next().ok_or_else(|| bad("missing numeric component"))?)?;
    }
    if w.next().is_some() {
        return Err(bad("extra numeric component"));
    }
    Ok(a)
}
#[derive(Default)]
struct Budget {
    floats: usize,
    indices: usize,
    sources: usize,
    triangles: usize,
}
fn numeric_array(node: &Node, b: &mut Budget, cancel: &AtomicBool) -> Result<Vec<f64>, RobotError> {
    let count = integer(node.attr("count")?, MAX_ARRAY)?;
    b.floats = b
        .floats
        .checked_add(count)
        .filter(|n| *n <= MAX_FLOATS)
        .ok_or_else(|| limit("aggregate numeric array budget exceeded"))?;
    let mut out = Vec::with_capacity(count);
    for word in node
        .leaf(&["id", "count", "name", "digits", "magnitude"])?
        .split_whitespace()
    {
        cancelled(cancel)?;
        if out.len() == count {
            return Err(bad("float array exceeds declared count"));
        }
        out.push(number(word)?);
    }
    if out.len() != count {
        return Err(bad("float array count mismatch"));
    }
    Ok(out)
}
fn indices(node: &Node, b: &mut Budget, cancel: &AtomicBool) -> Result<Vec<usize>, RobotError> {
    let mut out = Vec::new();
    for word in node.leaf(&[])?.split_whitespace() {
        cancelled(cancel)?;
        if b.indices == MAX_INDICES {
            return Err(limit("aggregate index budget exceeded"));
        }
        b.indices += 1;
        out.push(integer(word, MAX_RECORDS)?);
    }
    Ok(out)
}
struct Source {
    values: Vec<f64>,
    offset: usize,
    count: usize,
    stride: usize,
    params: Vec<Option<String>>,
}
impl Source {
    fn read(&self, i: usize, semantic: &str) -> Result<[f64; 3], RobotError> {
        if i >= self.count {
            return Err(bad("attribute index outside accessor records"));
        }
        let channels: &[&str] = if semantic == "TEXCOORD" {
            &["S", "T"]
        } else {
            &["X", "Y", "Z"]
        };
        let mut p = [0.; 3];
        for (slot, channel) in channels.iter().enumerate() {
            let c = self
                .params
                .iter()
                .position(|n| n.as_deref() == Some(channel))
                .or_else(|| {
                    if semantic == "TEXCOORD" {
                        self.params
                            .iter()
                            .position(|n| n.as_deref() == Some(if slot == 0 { "U" } else { "V" }))
                    } else {
                        None
                    }
                })
                .ok_or_else(|| unsupported("accessor needs named X/Y/Z or S/T (U/V) channels"))?;
            p[slot] = *self
                .values
                .get(self.offset + i * self.stride + c)
                .ok_or_else(|| bad("accessor points outside float array"))?;
        }
        Ok(p)
    }
}
fn source(node: &Node, b: &mut Budget, cancel: &AtomicBool) -> Result<Source, RobotError> {
    b.sources += 1;
    if b.sources > MAX_SOURCES {
        return Err(limit("more than 512 sources"));
    }
    node.only(&["float_array", "technique_common"], &["id", "name"])?;
    let array = node.required("float_array")?;
    let array_id = array.attr("id")?;
    let values = numeric_array(array, b, cancel)?;
    let common = node.required("technique_common")?;
    common.only(&["accessor"], &[])?;
    let a = common.required("accessor")?;
    a.only(&["param"], &["source", "count", "stride", "offset"])?;
    if reference(a.attr("source")?)? != array_id {
        return Err(bad("accessor references another array"));
    }
    let count = integer(a.attr("count")?, MAX_RECORDS)?;
    let stride = integer(a.optional("stride").unwrap_or("1"), 8)?;
    let offset = integer(a.optional("offset").unwrap_or("0"), MAX_ARRAY)?;
    if stride == 0 || a.children.is_empty() || a.children.len() > stride {
        return Err(bad("invalid accessor stride/params"));
    }
    let end = if count == 0 {
        Some(offset)
    } else {
        (count - 1)
            .checked_mul(stride)
            .and_then(|n| n.checked_add(offset))
            .and_then(|n| n.checked_add(a.children.len()))
    };
    if end.is_none_or(|n| n > values.len()) {
        return Err(bad("accessor range exceeds array"));
    }
    let mut params = Vec::new();
    for p in &a.children {
        p.only(&[], &["name", "type", "semantic"])?;
        if p.attr("type")? != "float" {
            return Err(unsupported("only float accessor params are supported"));
        }
        let name = p.optional("name").map(str::to_owned);
        if name
            .as_ref()
            .is_some_and(|n| params.contains(&Some(n.clone())))
        {
            return Err(bad("duplicate accessor parameter"));
        }
        params.push(name);
    }
    Ok(Source {
        values,
        offset,
        count,
        stride,
        params,
    })
}
#[derive(Clone)]
struct Input {
    semantic: String,
    source: String,
    offset: usize,
}
fn input(node: &Node, shared: bool) -> Result<Input, RobotError> {
    node.only(
        &[],
        if shared {
            &["semantic", "source", "offset", "set"]
        } else {
            &["semantic", "source"]
        },
    )?;
    let semantic = node.attr("semantic")?;
    if !["VERTEX", "POSITION", "NORMAL", "TEXCOORD"].contains(&semantic) {
        return Err(unsupported("unsupported mesh input semantic"));
    }
    if node.optional("set").is_some_and(|s| s != "0") {
        return Err(unsupported("only texture/normal set 0 is supported"));
    }
    Ok(Input {
        semantic: semantic.into(),
        source: reference(node.attr("source")?)?.into(),
        offset: integer(node.optional("offset").unwrap_or("0"), 7)?,
    })
}
struct Primitive {
    triangles: Vec<Triangle>,
    uvs: Option<Vec<TriangleUv>>,
    symbol: Option<String>,
}
struct Geometry {
    parts: Vec<Primitive>,
}
fn geometry(node: &Node, b: &mut Budget, cancel: &AtomicBool) -> Result<Geometry, RobotError> {
    node.only(&["mesh", "extra"], &["id", "name"])?;
    let m = node.required("mesh")?;
    m.only(
        &["source", "vertices", "triangles", "polylist", "extra"],
        &[],
    )?;
    let mut sources = BTreeMap::new();
    for n in m.children.iter().filter(|n| n.tag == "source") {
        sources.insert(n.attr("id")?.to_owned(), source(n, b, cancel)?);
    }
    let mut vertices = BTreeMap::new();
    for n in m.children.iter().filter(|n| n.tag == "vertices") {
        n.only(&["input"], &["id", "name"])?;
        if n.children.is_empty() || n.children.len() > 3 {
            return Err(bad("vertices requires 1–3 distinct attribute inputs"));
        }
        let list = n
            .children
            .iter()
            .map(|v| input(v, false))
            .collect::<Result<Vec<_>, _>>()?;
        if list
            .iter()
            .any(|i| i.semantic == "VERTEX" || !sources.contains_key(&i.source))
            || list
                .iter()
                .enumerate()
                .any(|(index, i)| list[..index].iter().any(|old| old.semantic == i.semantic))
        {
            return Err(bad("invalid vertices binding"));
        }
        vertices.insert(n.attr("id")?.to_owned(), list);
    }
    let mut parts = Vec::new();
    for n in m
        .children
        .iter()
        .filter(|n| n.tag == "triangles" || n.tag == "polylist")
    {
        cancelled(cancel)?;
        if parts.len() == MAX_VISUALS {
            return Err(limit("more than 256 primitive parts"));
        }
        n.only(
            &["input", "p", "vcount", "extra"],
            &["count", "material", "name"],
        )?;
        let face_count = integer(n.attr("count")?, MAX_ROBOT_TRIANGLES)?;
        let input_count = n.children.iter().filter(|n| n.tag == "input").count();
        if input_count == 0 || input_count > 8 {
            return Err(bad("mesh needs 1–8 inputs"));
        }
        let shared = n
            .children
            .iter()
            .filter(|n| n.tag == "input")
            .map(|n| input(n, true))
            .collect::<Result<Vec<_>, _>>()?;
        let stride = shared.iter().map(|i| i.offset).max().unwrap() + 1;
        let mut inputs = Vec::new();
        for i in shared {
            if i.semantic == "VERTEX" {
                let v = vertices
                    .get(&i.source)
                    .ok_or_else(|| bad("missing vertices source"))?;
                inputs.extend(v.iter().cloned().map(|mut v| {
                    v.offset = i.offset;
                    v
                }));
            } else {
                inputs.push(i);
            }
        }
        let position = inputs
            .iter()
            .position(|i| i.semantic == "POSITION")
            .ok_or_else(|| bad("missing position input"))?;
        for (i, entry) in inputs.iter().enumerate() {
            if inputs[..i].iter().any(|old| old.semantic == entry.semantic) {
                return Err(bad("duplicate mesh semantic"));
            }
            if !sources.contains_key(&entry.source) {
                return Err(bad("missing attribute source"));
            }
        }
        let corners = if n.tag == "triangles" {
            if n.one("vcount")?.is_some() {
                return Err(bad("triangles cannot contain vcount"));
            }
            vec![3; face_count]
        } else {
            let v = indices(n.required("vcount")?, b, cancel)?;
            if v.len() != face_count || v.iter().any(|n| *n < 3 || *n > 256) {
                return Err(bad("polylist count/arity mismatch; maximum 256 corners"));
            }
            v
        };
        let expanded = corners
            .iter()
            .try_fold(0usize, |n, c| n.checked_add(c - 2))
            .ok_or_else(|| limit("face expansion overflow"))?;
        b.triangles = b
            .triangles
            .checked_add(expanded)
            .filter(|n| *n <= MAX_ROBOT_TRIANGLES)
            .ok_or_else(|| limit("source geometry exceeds 50000 triangles"))?;
        let words = indices(n.required("p")?, b, cancel)?;
        let expected = corners
            .iter()
            .try_fold(0usize, |n, c| n.checked_add(*c))
            .and_then(|n| n.checked_mul(stride))
            .ok_or_else(|| limit("index size overflow"))?;
        if words.len() != expected {
            return Err(bad("primitive index count mismatch"));
        }
        let mut triangles = Vec::with_capacity(expanded);
        let has_uv = inputs.iter().any(|i| i.semantic == "TEXCOORD");
        let mut uvs = has_uv.then(|| Vec::with_capacity(expanded));
        let mut cursor = 0;
        for count in corners {
            cancelled(cancel)?;
            let mut points = Vec::with_capacity(count);
            let mut uv_corners = Vec::with_capacity(if has_uv { count } else { 0 });
            for _ in 0..count {
                let mut point = None;
                for (j, i) in inputs.iter().enumerate() {
                    let p = sources[&i.source].read(words[cursor + i.offset], &i.semantic)?;
                    if j == position {
                        point = Some(p);
                    }
                    if i.semantic == "TEXCOORD" {
                        uv_corners.push([p[0] as f32, p[1] as f32]);
                    }
                }
                points.push(point.ok_or_else(|| bad("missing position"))?);
                cursor += stride;
            }
            triangles.extend(mesh::triangulate(&points)?);
            if let Some(uvs) = &mut uvs {
                uvs.extend(
                    (1..uv_corners.len() - 1)
                        .map(|i| [uv_corners[0], uv_corners[i], uv_corners[i + 1]]),
                );
            }
        }
        let symbol = n
            .optional("material")
            .map(|s| {
                id(s)?;
                Ok::<_, RobotError>(s.to_owned())
            })
            .transpose()?;
        parts.push(Primitive {
            triangles,
            symbol,
            uvs,
        });
    }
    if parts.is_empty() {
        return Err(bad("mesh contains no supported primitives"));
    }
    Ok(Geometry { parts })
}
fn color(n: &Node) -> Result<[f32; 4], RobotError> {
    let v = fixed::<4>(n.leaf(&["sid"])?)?;
    if v.iter().any(|v| !(0.0..=1.0).contains(v)) {
        return Err(bad("material color outside [0,1]"));
    }
    Ok(v.map(|v| v as f32))
}
fn images(root: &Node) -> Result<BTreeMap<String, String>, RobotError> {
    let mut out = BTreeMap::new();
    if let Some(lib) = root.one("library_images")? {
        lib.only(&["image", "extra"], &["id", "name"])?;
        for image in lib.children.iter().filter(|n| n.tag == "image") {
            if out.len() == 16 {
                return Err(limit("more than 16 declared images"));
            }
            image.only(&["init_from", "extra"], &["id", "name", "format"])?;
            let path = image.required("init_from")?.leaf(&[])?;
            if path.is_empty()
                || path.len() > super::MAX_PATH_BYTES
                || path.contains([':', '\\'])
                || path.starts_with('/')
                || path.chars().any(char::is_control)
            {
                return Err(unsupported("image must reference a relative local file"));
            }
            out.insert(image.attr("id")?.into(), path.into());
        }
    }
    Ok(out)
}
fn sampler_image(
    profile: &Node,
    name: &str,
    images: &BTreeMap<String, String>,
) -> Result<String, RobotError> {
    id(name)?;
    let mut surfaces = BTreeMap::new();
    let mut samplers = BTreeMap::new();
    let mut seen = std::collections::BTreeSet::new();
    for p in profile.children.iter().filter(|n| n.tag == "newparam") {
        if seen.len() == 256 {
            return Err(limit("more than 256 effect parameters"));
        }
        let sid = p.attr("sid")?;
        id(sid)?;
        if !seen.insert(sid) {
            return Err(bad("duplicate effect parameter sid"));
        }
        p.only(&["surface", "sampler2D"], &["sid"])?;
        if p.children.len() != 1 {
            return Err(bad("newparam requires one surface or sampler2D"));
        }
        if let Some(s) = p.one("surface")? {
            s.only(&["init_from", "format"], &["type"])?;
            if s.attr("type")? != "2D" {
                return Err(unsupported("only 2D image surfaces are supported"));
            }
            let image = s.required("init_from")?.leaf(&[])?;
            id(image)?;
            if !images.contains_key(image) {
                return Err(bad("surface references a missing image"));
            }
            if let Some(format) = s.one("format")? {
                let _ = format.leaf(&[])?;
            }
            surfaces.insert(sid, image);
        } else {
            let s = p.required("sampler2D")?;
            s.only(
                &["source", "wrap_s", "wrap_t", "minfilter", "magfilter"],
                &[],
            )?;
            let surface = s.required("source")?.leaf(&[])?;
            id(surface)?;
            for tag in ["wrap_s", "wrap_t", "minfilter", "magfilter"] {
                if let Some(value) = s.one(tag)? {
                    if value.leaf(&[])?
                        != if tag.starts_with("wrap") {
                            "WRAP"
                        } else {
                            "LINEAR"
                        }
                    {
                        return Err(unsupported(
                            "sampler requires repeat wrapping and LINEAR filtering",
                        ));
                    }
                }
            }
            samplers.insert(sid, surface);
        }
    }
    let surface = samplers
        .get(name)
        .ok_or_else(|| bad("diffuse texture references a missing sampler"))?;
    let image = surfaces
        .get(surface)
        .ok_or_else(|| bad("sampler references a missing surface"))?;
    Ok(images
        .get(*image)
        .ok_or_else(|| bad("missing image"))?
        .clone())
}
fn effects(root: &Node) -> Result<BTreeMap<String, Material>, RobotError> {
    let images = images(root)?;
    let mut out = BTreeMap::new();
    if let Some(lib) = root.one("library_effects")? {
        lib.only(&["effect", "extra"], &["id", "name"])?;
        for e in lib.children.iter().filter(|n| n.tag == "effect") {
            if out.len() == MAX_MATERIALS {
                return Err(limit("more than 128 effects"));
            }
            e.only(&["profile_COMMON", "extra"], &["id", "name"])?;
            let p = e.required("profile_COMMON")?;
            p.only(&["technique", "newparam", "extra"], &["id"])?;
            let mut parameters = std::collections::BTreeSet::new();
            for parameter in p.children.iter().filter(|n| n.tag == "newparam") {
                if parameters.len() == 256 {
                    return Err(limit("more than256 effect parameters"));
                }
                let sid = parameter.attr("sid")?;
                id(sid)?;
                if !parameters.insert(sid) {
                    return Err(bad("duplicate effect parameter sid"));
                }
            }
            let t = p.required("technique")?;
            t.only(
                &["lambert", "phong", "blinn", "constant", "extra"],
                &["sid", "id"],
            )?;
            let shaders = t
                .children
                .iter()
                .filter(|n| n.tag != "extra")
                .collect::<Vec<_>>();
            if shaders.len() != 1 {
                return Err(bad("effect needs one common shader"));
            }
            let shader = shaders[0];
            shader.only(
                &[
                    "diffuse",
                    "emission",
                    "ambient",
                    "specular",
                    "shininess",
                    "transparent",
                    "transparency",
                    "index_of_refraction",
                ],
                &[],
            )?;
            let mut rgba = [0.7, 0.7, 0.7, 1.];
            let mut texture = None;
            let mut opacity = 1.;
            for tag in [
                "diffuse",
                "emission",
                "ambient",
                "specular",
                "shininess",
                "transparent",
                "transparency",
                "index_of_refraction",
            ] {
                let _ = shader.one(tag)?;
            }
            for property in &shader.children {
                match property.tag.as_str() {
                    "diffuse" | "emission" | "ambient" | "specular" => {
                        if property.one("texture")?.is_some() {
                            if property.tag != "diffuse" {
                                return Err(unsupported(
                                    "only diffuse texture effects are supported",
                                ));
                            }
                            property.only(&["texture"], &[])?;
                            let t = property.required("texture")?;
                            t.only(&[], &["texture", "texcoord"])?;
                            id(t.attr("texcoord")?)?;
                            texture = Some(TextureReference {
                                filename: sampler_image(p, t.attr("texture")?, &images)?,
                                material_file: None,
                                semantic: Some(t.attr("texcoord")?.into()),
                            });
                            rgba = [1.; 4];
                            continue;
                        }
                        property.only(&["color"], &[])?;
                        let c = color(property.required("color")?)?;
                        if property.tag == "diffuse"
                            || (shader.tag == "constant" && property.tag == "emission")
                        {
                            rgba = c;
                        }
                    }
                    "shininess" | "transparency" | "index_of_refraction" => {
                        property.only(&["float"], &[])?;
                        let [v] = fixed::<1>(property.required("float")?.leaf(&["sid"])?)?;
                        if property.tag == "transparency" {
                            if !(0.0..=1.0).contains(&v) {
                                return Err(bad("transparency outside [0,1]"));
                            }
                            opacity *= v as f32;
                        }
                        if property.tag == "index_of_refraction" && v != 1. {
                            return Err(unsupported(
                                "transparency/refraction effect is unsupported",
                            ));
                        }
                    }
                    "transparent" => {
                        property.only(&["color"], &["opaque"])?;
                        if property.optional("opaque").unwrap_or("A_ONE") != "A_ONE" {
                            return Err(unsupported("transparent effect mode is unsupported"));
                        }
                        opacity *= color(property.required("color")?)?[3];
                    }
                    _ => unreachable!(),
                }
            }
            rgba[3] *= opacity;
            out.insert(e.attr("id")?.into(), Material { rgba, texture });
        }
    }
    Ok(out)
}
fn materials(root: &Node) -> Result<BTreeMap<String, Material>, RobotError> {
    let effects = effects(root)?;
    let mut out = BTreeMap::new();
    if let Some(lib) = root.one("library_materials")? {
        lib.only(&["material", "extra"], &["id", "name"])?;
        for m in lib.children.iter().filter(|n| n.tag == "material") {
            if out.len() == MAX_MATERIALS {
                return Err(limit("more than 128 materials"));
            }
            m.only(&["instance_effect", "extra"], &["id", "name"])?;
            let e = m.required("instance_effect")?;
            e.only(&[], &["url", "sid", "name"])?;
            let effect = effects
                .get(reference(e.attr("url")?)?)
                .ok_or_else(|| bad("missing material effect"))?;
            out.insert(m.attr("id")?.into(), effect.clone());
        }
    }
    Ok(out)
}
#[derive(Clone, Copy, Debug)]
struct Matrix([[f64; 4]; 4]);
impl Matrix {
    fn identity() -> Self {
        Self(std::array::from_fn(|i| {
            std::array::from_fn(|j| if i == j { 1. } else { 0. })
        }))
    }
    fn compose(self, b: Self) -> Result<Self, RobotError> {
        let out = Self(std::array::from_fn(|i| {
            std::array::from_fn(|j| (0..4).map(|k| self.0[i][k] * b.0[k][j]).sum::<f64>())
        }));
        if out
            .0
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || v.abs() > MAX_COORDINATE)
        {
            return Err(limit("composed transform exceeds coordinate bound"));
        }
        Ok(out)
    }
    fn point(self, p: [f64; 3], unit: f64) -> Result<[f64; 3], RobotError> {
        let p = std::array::from_fn(|i| {
            (self.0[i][3] + (0..3).map(|j| self.0[i][j] * p[j]).sum::<f64>()) * unit
        });
        if p.iter().any(|v| !v.is_finite() || v.abs() > MAX_COORDINATE) {
            return Err(limit("instanced geometry exceeds coordinate bound"));
        }
        Ok(p)
    }
}
fn transform(n: &Node) -> Result<Matrix, RobotError> {
    let mut m = Matrix::identity();
    for t in &n.children {
        let mut v = Matrix::identity();
        match t.tag.as_str() {
            "translate" => {
                let p = fixed::<3>(t.leaf(&["sid"])?)?;
                for (i, p) in p.into_iter().enumerate() {
                    v.0[i][3] = p;
                }
            }
            "scale" => {
                let p = fixed::<3>(t.leaf(&["sid"])?)?;
                for (i, p) in p.into_iter().enumerate() {
                    if p == 0. || p.abs() > MAX_DIMENSION {
                        return Err(bad("invalid node scale"));
                    }
                    v.0[i][i] = p;
                }
            }
            "rotate" => {
                let [x, y, z, degrees] = fixed::<4>(t.leaf(&["sid"])?)?;
                let norm = (x * x + y * y + z * z).sqrt();
                if norm == 0. {
                    return Err(bad("rotation axis is zero"));
                }
                let a = [x / norm, y / norm, z / norm];
                let (s, c) = degrees.to_radians().sin_cos();
                for i in 0..3 {
                    for j in 0..3 {
                        v.0[i][j] = a[i] * a[j] * (1. - c) + if i == j { c } else { 0. };
                    }
                }
                v.0[0][1] -= a[2] * s;
                v.0[0][2] += a[1] * s;
                v.0[1][0] += a[2] * s;
                v.0[1][2] -= a[0] * s;
                v.0[2][0] -= a[1] * s;
                v.0[2][1] += a[0] * s;
            }
            "matrix" => {
                let p = fixed::<16>(t.leaf(&["sid"])?)?;
                v = Matrix(std::array::from_fn(|i| {
                    std::array::from_fn(|j| p[i * 4 + j])
                }));
                if v.0[3] != [0., 0., 0., 1.] {
                    return Err(unsupported("non-affine node matrix"));
                }
            }
            _ => continue,
        }
        m = m.compose(v)?;
    }
    Ok(m)
}
struct Scene<'a> {
    geometries: &'a BTreeMap<String, Geometry>,
    materials: &'a BTreeMap<String, Material>,
    nodes: BTreeMap<String, &'a Node>,
    unit: f64,
    cancel: &'a AtomicBool,
    visits: usize,
    triangles: usize,
    parts: Vec<Part>,
}
impl Scene<'_> {
    fn instance(&mut self, n: &Node, m: Matrix) -> Result<(), RobotError> {
        n.only(&["bind_material", "extra"], &["url", "sid", "name"])?;
        let g = self
            .geometries
            .get(reference(n.attr("url")?)?)
            .ok_or_else(|| bad("missing geometry instance"))?;
        let mut bindings = BTreeMap::new();
        if let Some(b) = n.one("bind_material")? {
            b.only(&["technique_common", "extra"], &[])?;
            let common = b.required("technique_common")?;
            common.only(&["instance_material"], &[])?;
            for i in &common.children {
                i.only(
                    &["bind_vertex_input", "extra"],
                    &["symbol", "target", "sid", "name"],
                )?;
                for v in i.children.iter().filter(|n| n.tag == "bind_vertex_input") {
                    v.only(&[], &["semantic", "input_semantic", "input_set"])?;
                    id(v.attr("semantic")?)?;
                    if v.attr("input_semantic")? != "TEXCOORD"
                        || v.optional("input_set").unwrap_or("0") != "0"
                    {
                        return Err(unsupported("only TEXCOORD binding set 0 is supported"));
                    }
                }
                let symbol = i.attr("symbol")?;
                id(symbol)?;
                let color = self
                    .materials
                    .get(reference(i.attr("target")?)?)
                    .ok_or_else(|| bad("missing bound material"))?;
                if let Some(semantic) = color.texture.as_ref().and_then(|t| t.semantic.as_ref()) {
                    let matching = i
                        .children
                        .iter()
                        .filter(|v| {
                            v.tag == "bind_vertex_input"
                                && v.optional("semantic") == Some(semantic.as_str())
                        })
                        .count();
                    if matching != 1 {
                        return Err(bad("diffuse texture needs one matching TEXCOORD binding"));
                    }
                }
                if bindings.insert(symbol, color.clone()).is_some() {
                    return Err(bad("duplicate material symbol binding"));
                }
            }
        }
        for p in &g.parts {
            cancelled(self.cancel)?;
            if self.parts.len() == MAX_VISUALS {
                return Err(limit("instanced parts exceed 256"));
            }
            self.triangles = self
                .triangles
                .checked_add(p.triangles.len())
                .filter(|n| *n <= MAX_ROBOT_TRIANGLES)
                .ok_or_else(|| limit("instanced geometry exceeds 50000 triangles"))?;
            let material = p
                .symbol
                .as_ref()
                .map(|s| {
                    bindings
                        .get(s.as_str())
                        .cloned()
                        .ok_or_else(|| bad("primitive material symbol is not bound"))
                })
                .transpose()?;
            let mut triangles = Vec::with_capacity(p.triangles.len());
            for t in &p.triangles {
                cancelled(self.cancel)?;
                let mut points = [[0.; 3]; 3];
                for (dest, src) in points.iter_mut().zip(t.0) {
                    *dest = m.point(src, self.unit)?;
                }
                triangles.push(Triangle(points));
            }
            self.parts.push(Part {
                texture: material.as_ref().and_then(|m| m.texture.clone()),
                triangles,
                rgba: material.map(|m| m.rgba),
                uvs: p.uvs.clone(),
            });
        }
        Ok(())
    }
    fn node(
        &mut self,
        n: &Node,
        parent: Matrix,
        stack: &mut Vec<String>,
        depth: usize,
    ) -> Result<(), RobotError> {
        cancelled(self.cancel)?;
        self.visits += 1;
        if self.visits > MAX_NODE_VISITS || depth > 32 {
            return Err(limit("expanded scene node visits/depth exceeded"));
        }
        n.only(
            &[
                "translate",
                "rotate",
                "scale",
                "matrix",
                "instance_geometry",
                "instance_node",
                "node",
                "extra",
            ],
            &["id", "sid", "name", "type", "layer"],
        )?;
        if n.optional("type").is_some_and(|t| t != "NODE") {
            return Err(unsupported("joint/controller scene nodes are unsupported"));
        }
        let m = parent.compose(transform(n)?)?;
        for child in &n.children {
            match child.tag.as_str() {
                "instance_geometry" => self.instance(child, m)?,
                "node" => self.node(child, m, stack, depth + 1)?,
                "instance_node" => {
                    child.only(&["extra"], &["url", "sid", "name"])?;
                    let id = reference(child.attr("url")?)?;
                    if stack.iter().any(|n| n == id) {
                        return Err(bad("cyclic node instance"));
                    }
                    let target = *self
                        .nodes
                        .get(id)
                        .ok_or_else(|| bad("missing node instance"))?;
                    stack.push(id.into());
                    self.node(target, m, stack, depth + 1)?;
                    stack.pop();
                }
                _ => {}
            }
        }
        Ok(())
    }
}
fn unit(root: &Node) -> Result<f64, RobotError> {
    let mut unit = 1.;
    if let Some(asset) = root.one("asset")? {
        asset.only(
            &[
                "contributor",
                "created",
                "modified",
                "keywords",
                "revision",
                "subject",
                "title",
                "unit",
                "up_axis",
            ],
            &[],
        )?;
        if let Some(n) = asset.one("unit")? {
            n.only(&[], &["meter", "name"])?;
            unit = number(n.optional("meter").unwrap_or("1"))?;
            if unit <= 0. || unit > MAX_DIMENSION {
                return Err(bad("unit metre scale must be positive and at most 10000"));
            }
        }
        if let Some(n) = asset.one("up_axis")? {
            if !["X_UP", "Y_UP", "Z_UP"].contains(&n.leaf(&[])?) {
                return Err(bad("invalid up_axis"));
            }
        }
    }
    Ok(unit)
}
fn collect_nodes<'a>(parent: &'a Node, nodes: &mut BTreeMap<String, &'a Node>) {
    for node in parent.children.iter().filter(|n| n.tag == "node") {
        if let Some(id) = node.optional("id") {
            nodes.insert(id.into(), node);
        }
        collect_nodes(node, nodes);
    }
}
pub(super) fn decode(bytes: &[u8], cancel: &AtomicBool) -> Result<Decoded, RobotError> {
    let root = xml::parse(bytes, cancel)?;
    if root.tag != "COLLADA" {
        return Err(bad("root must be COLLADA"));
    }
    root.only(
        &[
            "asset",
            "library_geometries",
            "library_materials",
            "library_images",
            "library_effects",
            "library_visual_scenes",
            "library_nodes",
            "scene",
            "extra",
        ],
        &["version", "xmlns"],
    )?;
    if !["1.4.1", "1.5.0", "1.5.1"].contains(&root.attr("version")?) {
        return Err(unsupported("unsupported document version"));
    }
    if root.optional("xmlns").is_some_and(|n| {
        ![
            "http://www.collada.org/2005/11/COLLADASchema",
            "http://www.collada.org/2008/03/COLLADASchema",
        ]
        .contains(&n)
    }) {
        return Err(unsupported("unknown default namespace"));
    }
    let unit = unit(&root)?;
    let materials = materials(&root)?;
    let mut budget = Budget::default();
    let mut geometries = BTreeMap::new();
    let lib = root.required("library_geometries")?;
    lib.only(&["geometry", "extra"], &["id", "name"])?;
    for g in lib.children.iter().filter(|n| n.tag == "geometry") {
        cancelled(cancel)?;
        if geometries.len() == MAX_GEOMETRIES {
            return Err(limit("more than 128 geometries"));
        }
        geometries.insert(g.attr("id")?.into(), geometry(g, &mut budget, cancel)?);
    }
    let visual_lib = root.required("library_visual_scenes")?;
    visual_lib.only(&["visual_scene", "extra"], &["id", "name"])?;
    let scene = root.required("scene")?;
    scene.only(&["instance_visual_scene", "extra"], &[])?;
    let selected = scene.required("instance_visual_scene")?;
    selected.only(&["extra"], &["url", "sid", "name"])?;
    let id = reference(selected.attr("url")?)?;
    let visual = visual_lib
        .children
        .iter()
        .find(|n| n.tag == "visual_scene" && n.optional("id") == Some(id))
        .ok_or_else(|| bad("missing selected visual scene"))?;
    visual.only(&["node", "extra"], &["id", "name"])?;
    let mut nodes = BTreeMap::new();
    for scene in visual_lib
        .children
        .iter()
        .filter(|n| n.tag == "visual_scene")
    {
        collect_nodes(scene, &mut nodes);
    }
    if let Some(lib) = root.one("library_nodes")? {
        lib.only(&["node", "extra"], &["id", "name"])?;
        collect_nodes(lib, &mut nodes);
    }
    let mut scene = Scene {
        geometries: &geometries,
        materials: &materials,
        nodes,
        unit,
        cancel,
        visits: 0,
        triangles: 0,
        parts: Vec::new(),
    };
    for node in visual.children.iter().filter(|n| n.tag == "node") {
        scene.node(node, Matrix::identity(), &mut Vec::new(), 1)?;
    }
    if scene.triangles == 0 {
        return Err(bad("selected scene contains no triangles"));
    }
    Ok(Decoded {
        parts: scene.parts,
        material_count: materials.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) const FIXTURE: &str =
        include_str!("../../../../fixtures/robot_visualizer/materials/panels.dae");
    fn read(s: &str) -> Result<Decoded, RobotError> {
        decode(s.as_bytes(), &AtomicBool::new(false))
    }
    #[test]
    fn collada_a_one_transparency_multiplies_material_alpha() {
        let s=FIXTURE.replace("0.95 0.2 0.12 1</color>","0.95 0.2 0.12 0.5</color>").replace("</diffuse>","</diffuse><transparent opaque=\"A_ONE\"><color>1 1 1 0.2</color></transparent><transparency><float>0.5</float></transparency>");
        let d = read(&s).unwrap();
        assert!((d.parts[0].rgba.unwrap()[3] - 0.05).abs() < 1e-6);
        assert!(read(&s.replace("A_ONE", "RGB_ZERO")).is_err());
        assert!(read(&s.replace("<float>0.5</float>", "<float>1.5</float>")).is_err());
    }
    #[test]
    fn diffuse_images_resolve_complete_reference_chain_and_matching_uv_binding() {
        let s = include_str!("../../../../fixtures/robot_visualizer/materials/textured.dae");
        let d = read(s).unwrap();
        let texture = d.parts[0].texture.as_ref().unwrap();
        assert_eq!(texture.filename, "textures/checker.png");
        assert_eq!(texture.semantic.as_deref(), Some("UVMap"));
        assert!(texture.material_file.is_none());
        assert_eq!(d.parts[0].rgba, Some([1.; 4]));
        assert!(d.parts[0].uvs.is_some());
        assert!(d.parts[1].texture.is_none());
        for (a, b) in [
            ("texture=\"checkerSampler\"", "texture=\"missing\""),
            (
                "<source>checkerSurface</source>",
                "<source>missing</source>",
            ),
            (
                "<init_from>checkerImage</init_from>",
                "<init_from>missing</init_from>",
            ),
            ("semantic=\"UVMap\"", "semantic=\"Other\""),
            ("<wrap_s>WRAP</wrap_s>", "<wrap_s>CLAMP</wrap_s>"),
            (
                "<minfilter>LINEAR</minfilter>",
                "<minfilter>NEAREST</minfilter>",
            ),
            ("textures/checker.png", "https://example/checker.png"),
            ("input_set=\"0\"", "input_set=\"1\""),
        ] {
            assert_ne!(s, s.replace(a, b));
            assert!(read(&s.replace(a, b)).is_err(), "{a}");
        }
        let binding =
            "<bind_vertex_input semantic=\"UVMap\" input_semantic=\"TEXCOORD\" input_set=\"0\"/>";
        assert!(read(&s.replace(binding, "")).is_err());
        assert!(read(&s.replace(binding, &format!("{binding}{binding}"))).is_err());
        let param="<newparam sid=\"checkerSampler\"><sampler2D><source>checkerSurface</source><wrap_s>WRAP</wrap_s><wrap_t>WRAP</wrap_t><minfilter>LINEAR</minfilter><magfilter>LINEAR</magfilter></sampler2D></newparam>";
        assert!(read(&s.replace(param, &format!("{param}{param}"))).is_err());
    }
    fn close(a: [f64; 3], b: [f64; 3]) {
        for (a, b) in a.into_iter().zip(b) {
            assert!((a - b).abs() < 1e-10, "{a} != {b}");
        }
    }
    #[test]
    fn indexed_polylist_material_binding_instancing_units_and_ordered_transforms() {
        let d = read(FIXTURE).unwrap();
        assert_eq!(d.material_count, 2);
        assert_eq!(d.parts.len(), 2);
        assert_eq!(d.parts[0].triangles.len(), 2);
        assert_eq!(d.parts[1].triangles.len(), 2);
        assert_eq!(d.parts[0].rgba, Some([0.95, 0.2, 0.12, 1.]));
        assert_eq!(d.parts[1].rgba, Some([0.05, 0.75, 0.65, 1.]));
        assert_eq!(
            d.parts[0].uvs.as_deref(),
            Some(
                &[
                    [[0., 0.], [1., 0.], [1., 1.]],
                    [[0., 0.], [1., 1.], [0., 1.]]
                ][..]
            )
        );
        assert_eq!(d.parts[0].uvs, d.parts[1].uvs);
        close(d.parts[0].triangles[0].0[0], [1., 0., 0.25]);
        close(d.parts[0].triangles[0].0[1], [1., 2., 0.25]);
        close(d.parts[0].triangles[0].0[2], [0.5, 2., 0.25]);
        close(d.parts[1].triangles[0].0[0], [-1., 0., 0.]);
        close(d.parts[1].triangles[0].0[2], [0., 0.5, 0.]);
        // RViz intentionally ignores valid up_axis metadata, including the default Y_UP.
        assert_eq!(
            read(&FIXTURE.replace("Y_UP", "Z_UP")).unwrap().parts[0].triangles,
            d.parts[0].triangles
        );
        let triangle = FIXTURE
            .replace("<polylist count=\"1\"", "<triangles count=\"2\"")
            .replace("<vcount>4</vcount>", "")
            .replace(
                "<p>0 0 0 1 0 1 2 0 2 3 0 3</p>",
                "<p>0 0 0 1 0 1 2 0 2 0 0 0 2 0 2 3 0 3</p>",
            )
            .replace("</polylist>", "</triangles>");
        assert_eq!(
            read(&triangle).unwrap().parts[0].triangles,
            d.parts[0].triangles
        );
    }
    #[test]
    fn rejects_malformed_arrays_accessors_indices_bindings_and_external_references() {
        for (from, to) in [
            ("count=\"17\"", "count=\"18\""),
            ("stride=\"4\"", "stride=\"8\""),
            ("offset=\"1\"><param", "offset=\"20\"><param"),
            ("source=\"#positionsArray\"", "source=\"#missing\""),
            ("<vcount>4</vcount>", "<vcount>2</vcount>"),
            ("<p>0 0 0", "<p>4 0 0"),
            ("#coralEffect", "#missingEffect"),
            ("symbol=\"body\"", "symbol=\"wrong\""),
            ("#panel\"", "https://example/panel\""),
            ("#reusable\"", "#missingNode\""),
            ("meter=\"0.001\"", "meter=\"0\""),
            ("Y_UP", "NOT_AN_AXIS"),
            ("99 0 0 0 7", "99 NaN 0 0 7"),
            ("<scale>2 1 1</scale>", "<scale>0 1 1</scale>"),
            ("set=\"0\"", "set=\"1\""),
            (
                "<diffuse>",
                "<diffuse><color>1 1 1 1</color></diffuse><diffuse>",
            ),
            ("semantic=\"NORMAL\"", "semantic=\"COLOR\""),
        ] {
            assert_ne!(
                FIXTURE,
                FIXTURE.replace(from, to),
                "test must change source: {from}"
            );
            assert!(read(&FIXTURE.replace(from, to)).is_err(), "{from} → {to}");
        }
    }
    #[test]
    fn unsupported_dynamic_texture_and_non_affine_content_never_yields_partial_model() {
        for (from, to) in [
            ("<mesh>", "<mesh><asset><unit meter=\"2\"/></asset>"),
            (
                "<library_effects>",
                "<library_animations/><library_effects>",
            ),
            ("<scale>2 1 1</scale>", "<skew>0 1 0 0 0 0 1</skew>"),
            (
                "<color>0.95 0.2 0.12 1</color>",
                "<texture texture=\"surface\" texcoord=\"UVMap\"/>",
            ),
            ("0 0 0 1</matrix>", "1 0 0 1</matrix>"),
            (
                "<node id=\"scaled\">",
                "<node id=\"scaled\" type=\"JOINT\">",
            ),
            (
                "<color>0.95 0.2 0.12 1</color>",
                "<color>0.95 0.2 0.12 1.5</color>",
            ),
        ] {
            assert!(read(&FIXTURE.replace(from, to)).is_err(), "{from} → {to}");
        }
    }
    #[test]
    fn xml_entities_duplicates_depth_and_counts_are_bounded_before_allocation() {
        for s in [
            "<!DOCTYPE COLLADA><COLLADA version=\"1.4.1\"/>",
            "<COLLADA version=\"1.4.1\">&x;</COLLADA>",
            "<COLLADA id=\"a\"><node id=\"a\"/></COLLADA>",
            "<COLLADA version=\"1.4.1\"><node></COLLADA>",
            "<COLLADA version=\"1.4.1\" version=\"1.5.0\"/>",
        ] {
            assert!(read(s).is_err());
        }
        assert_eq!(
            read(&format!("{}{}", "<node>".repeat(33), "</node>".repeat(33)))
                .unwrap_err()
                .kind,
            RobotErrorKind::Limit
        );
        for (from, to) in [
            ("count=\"17\"", "count=\"999999999999\""),
            ("count=\"4\" stride=\"4\"", "count=\"150001\" stride=\"4\""),
            ("offset=\"2\" set", "offset=\"8\" set"),
        ] {
            assert_eq!(
                read(&FIXTURE.replace(from, to)).unwrap_err().kind,
                RobotErrorKind::Limit
            );
        }
        assert_eq!(
            decode(FIXTURE.as_bytes(), &AtomicBool::new(true))
                .unwrap_err()
                .kind,
            RobotErrorKind::Cancelled
        );
    }
    #[test]
    fn cyclic_nodes_and_expanded_geometry_are_bounded() {
        let cycle = FIXTURE.replace(
            "<node id=\"reusable\">",
            "<node id=\"reusable\"><instance_node url=\"#reusable\"/>",
        );
        assert!(read(&cycle).unwrap_err().message.contains("cyclic"));
        let instances = "<instance_node url=\"#reusable\"/>".repeat(MAX_VISUALS);
        let max = FIXTURE.replace(
            "<node id=\"copy\">",
            &format!("<node id=\"copy\">{instances}"),
        );
        assert_eq!(read(&max).unwrap_err().kind, RobotErrorKind::Limit);
        let visits = "<node/>".repeat(MAX_NODE_VISITS);
        let max = FIXTURE.replace(
            "<visual_scene id=\"main\">",
            &format!("<visual_scene id=\"main\">{visits}"),
        );
        assert_eq!(read(&max).unwrap_err().kind, RobotErrorKind::Limit);
    }
}

#[cfg(test)]
mod budget_tests {
    use super::tests::FIXTURE;
    use super::*;
    fn read(s: &str) -> Result<Decoded, RobotError> {
        decode(s.as_bytes(), &AtomicBool::new(false))
    }
    fn selected_nodes(source: &str, nodes: &str) -> String {
        let start =
            source.find("<visual_scene id=\"main\">").unwrap() + "<visual_scene id=\"main\">".len();
        let end = source.find("</visual_scene>").unwrap();
        format!("{}{nodes}{}", &source[..start], &source[end..])
    }
    #[test]
    fn exact_and_over_expanded_material_parts_and_triangles() {
        let instances = "<instance_node url=\"#reusable\"/>".repeat(MAX_VISUALS);
        let s = selected_nodes(FIXTURE, &format!("<node>{instances}</node>"));
        assert_eq!(read(&s).unwrap().parts.len(), MAX_VISUALS);
        assert_eq!(
            read(&s.replace(
                "</visual_scene>",
                "<node><instance_node url=\"#reusable\"/></node></visual_scene>"
            ))
            .unwrap_err()
            .kind,
            RobotErrorKind::Limit
        );
        let one = selected_nodes(FIXTURE, "<node><instance_node url=\"#reusable\"/></node>");
        let words = "0 0 0 1 0 1 2 0 2 0 0 0 2 0 2 3 0 3 ".repeat(MAX_ROBOT_TRIANGLES / 2);
        let max = one
            .replace("<polylist count=\"1\"", "<triangles count=\"50000\"")
            .replace("<vcount>4</vcount>", "")
            .replace("<p>0 0 0 1 0 1 2 0 2 3 0 3</p>", &format!("<p>{words}</p>"))
            .replace("</polylist>", "</triangles>");
        let d = read(&max).unwrap();
        assert_eq!(d.parts[0].triangles.len(), MAX_ROBOT_TRIANGLES);
        assert_eq!(
            read(&max.replace("count=\"50000\"", "count=\"50001\""))
                .unwrap_err()
                .kind,
            RobotErrorKind::Limit
        );
    }
    #[test]
    fn numeric_and_index_budget_bounds_are_enforced_before_growth() {
        let cancel = AtomicBool::new(false);
        let mut b = Budget::default();
        for count in [MAX_ARRAY, MAX_ARRAY, MAX_FLOATS - MAX_ARRAY * 2] {
            let n = xml::parse(
                format!(
                    "<float_array count=\"{count}\">{}</float_array>",
                    "0 ".repeat(count)
                )
                .as_bytes(),
                &cancel,
            )
            .unwrap();
            assert_eq!(numeric_array(&n, &mut b, &cancel).unwrap().len(), count);
        }
        let n = xml::parse(b"<float_array count=\"1\">0</float_array>", &cancel).unwrap();
        assert_eq!(
            numeric_array(&n, &mut b, &cancel).unwrap_err().kind,
            RobotErrorKind::Limit
        );
        let mut b = Budget::default();
        let n = xml::parse(
            format!("<p>{}</p>", "0 ".repeat(MAX_INDICES)).as_bytes(),
            &cancel,
        )
        .unwrap();
        assert_eq!(indices(&n, &mut b, &cancel).unwrap().len(), MAX_INDICES);
        let n = xml::parse(b"<p>0</p>", &cancel).unwrap();
        assert_eq!(
            indices(&n, &mut b, &cancel).unwrap_err().kind,
            RobotErrorKind::Limit
        );
    }
    #[test]
    fn xml_element_and_attribute_limits_are_exact() {
        let cancel = AtomicBool::new(false);
        let max = format!(
            "<root>{}</root>",
            "<n/>".repeat(super::super::MAX_XML_ELEMENTS - 1)
        );
        assert!(xml::parse(max.as_bytes(), &cancel).is_ok());
        assert_eq!(
            xml::parse(max.replace("</root>", "<n/></root>").as_bytes(), &cancel)
                .unwrap_err()
                .kind,
            RobotErrorKind::Limit
        );
        let attrs = (0..32).map(|i| format!(" a{i}=\"v\"")).collect::<String>();
        assert!(xml::parse(format!("<n{attrs}/>").as_bytes(), &cancel).is_ok());
        assert_eq!(
            xml::parse(format!("<n{attrs} extra=\"v\"/>").as_bytes(), &cancel)
                .unwrap_err()
                .kind,
            RobotErrorKind::Limit
        );
    }
    #[test]
    fn nested_node_references_and_nested_unit_override_rejection() {
        let nested = FIXTURE
            .replace("<library_nodes>", "<library_nodes><node id=\"container\">")
            .replace("</library_nodes>", "</node></library_nodes>");
        assert_eq!(read(&nested).unwrap().parts.len(), 2);
        let invalid = FIXTURE.replace(
            "<node id=\"reusable\">",
            "<node id=\"reusable\"><asset><unit meter=\"2\"/></asset>",
        );
        assert_eq!(
            read(&invalid).unwrap_err().kind,
            RobotErrorKind::Unsupported
        );
    }
    #[test]
    fn exact_accessor_source_material_and_scene_visit_budgets() {
        let cancel = AtomicBool::new(false);
        let source_node=xml::parse(b"<source id=\"s\"><float_array id=\"a\" count=\"0\"/><technique_common><accessor source=\"#a\" count=\"0\"><param name=\"X\" type=\"float\"/></accessor></technique_common></source>",&cancel).unwrap();
        let mut b = Budget::default();
        for _ in 0..MAX_SOURCES {
            assert!(source(&source_node, &mut b, &cancel).is_ok());
        }
        assert_eq!(
            source(&source_node, &mut b, &cancel).err().unwrap().kind,
            RobotErrorKind::Limit
        );
        let array = "0 ".repeat(MAX_RECORDS * 3);
        let node=xml::parse(format!("<source id=\"s\"><float_array id=\"a\" count=\"450000\">{array}</float_array><technique_common><accessor source=\"#a\" count=\"150000\" stride=\"3\"><param name=\"X\" type=\"float\"/><param name=\"Y\" type=\"float\"/><param name=\"Z\" type=\"float\"/></accessor></technique_common></source>").as_bytes(),&cancel).unwrap();
        let data = source(&node, &mut Budget::default(), &cancel).unwrap();
        assert_eq!(data.read(MAX_RECORDS - 1, "POSITION").unwrap(), [0.; 3]);
        assert!(data.read(MAX_RECORDS, "POSITION").is_err());
        let added = (0..MAX_MATERIALS - 2)
            .map(|i| {
                format!(
                    "<material id=\"unused{i}\"><instance_effect url=\"#coralEffect\"/></material>"
                )
            })
            .collect::<String>();
        let max = FIXTURE.replace(
            "</library_materials>",
            &format!("{added}</library_materials>"),
        );
        assert_eq!(read(&max).unwrap().material_count, MAX_MATERIALS);
        let over=max.replace("</library_materials>","<material id=\"overflow\"><instance_effect url=\"#coralEffect\"/></material></library_materials>");
        assert_eq!(read(&over).unwrap_err().kind, RobotErrorKind::Limit);
        let empty = "<node/>".repeat(MAX_NODE_VISITS - 2);
        let max = selected_nodes(
            FIXTURE,
            &format!("{empty}<node><instance_node url=\"#reusable\"/></node>"),
        );
        assert_eq!(read(&max).unwrap().parts.len(), 1);
        assert_eq!(
            read(&max.replace("</visual_scene>", "<node/></visual_scene>"))
                .unwrap_err()
                .kind,
            RobotErrorKind::Limit
        );
    }
}
