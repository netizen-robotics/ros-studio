//! Explicit Tools operations, owned and progressed on the backend thread.
use crate::service_codec::NativeMessage;
use serde_json::{json, Value};
use std::{
    ffi::{c_char, c_void, CStr, CString},
    path::PathBuf,
    time::{Duration, Instant},
};
use studio_protocol::{
    session::{Command, Event},
    subscriptions::*,
    tools::{Operation, Update, MAX_OPERATIONS},
};
type Ptr = *mut c_void;
extern "C" {
    fn tp_create() -> Ptr;
    fn tp_destroy(p: Ptr);
    fn pp_open(
        p: Ptr,
        i: *const c_char,
        d: *const c_char,
        package: *const c_char,
        name: *const c_char,
        id: *const c_char,
        best: i32,
        topic: *const c_char,
        domain: usize,
        transient: i32,
    ) -> i32;
    fn pp_send(p: Ptr) -> i32;
    fn pp_matched(p: Ptr, count: *mut usize) -> i32;
    fn sp_schema(p: Ptr, response: i32) -> Ptr;
    fn sp_message(p: Ptr, response: i32) -> Ptr;
    fn sp_error(p: Ptr) -> *const c_char;
}
struct Publisher(Ptr);
impl Drop for Publisher {
    fn drop(&mut self) {
        unsafe { tp_destroy(self.0) }
    }
}
pub(crate) struct Libraries {
    pub intro: String,
    pub dispatch: String,
    pub generic: String,
    pub package: String,
    pub name: String,
    pub identifier: String,
}
pub(crate) fn libraries(typ: &str, bundle: &str) -> Result<Libraries, String> {
    let parts: Vec<_> = typ.split('/').collect();
    if parts.len() != 3
        || !["msg", "srv", "action"].contains(&parts[1])
        || !parts.iter().all(|p| crate::probe_bundle::identifier(p))
    {
        return Err("Expected package/msg|srv|action/Type".into());
    }
    let identifier = match std::env::var("RMW_IMPLEMENTATION").as_deref() {
        Ok("rmw_fastrtps_cpp") => "rosidl_typesupport_fastrtps_c",
        Ok("rmw_cyclonedds_cpp") => "rosidl_typesupport_introspection_c",
        _ => return Err("Select Fast DDS or Cyclone DDS".into()),
    };
    let roots = if bundle.is_empty() {
        std::env::split_paths(
            &std::env::var_os("AMENT_PREFIX_PATH").ok_or("Missing runtime prefixes")?,
        )
        .collect::<Vec<_>>()
    } else {
        vec![crate::probe_bundle::bundle(
            std::path::Path::new(bundle),
            parts[0],
        )?]
    };
    let library = |id: &str| -> Result<String, String> {
        for root in &roots {
            let file: PathBuf = root.join("lib").join(format!(
                "lib{}__{}.{}",
                parts[0],
                id,
                std::env::consts::DLL_EXTENSION
            ));
            if file.is_file() {
                return Ok(file.to_string_lossy().into_owned());
            }
        }
        Err(format!(
            "Missing local {id} for {}. ROS discovery does not transfer interface libraries. Import a bundle matching this ROS distribution and platform, then retry Load schema",
            parts[0]
        ))
    };
    Ok(Libraries {
        intro: library("rosidl_typesupport_introspection_c")?,
        dispatch: library(identifier)?,
        generic: library("rosidl_typesupport_c")?,
        package: parts[0].into(),
        name: parts[2].into(),
        identifier: identifier.into(),
    })
}
impl Publisher {
    fn open(
        topic: &str,
        typ: &str,
        bundle: &str,
        value: &Value,
        best: bool,
        transient: bool,
    ) -> Result<Self, String> {
        let l = libraries(typ, bundle)?;
        // Validate all fields and bounds before opening any ROS endpoint.
        let mut validation = NativeMessage::load(&l.intro, &l.package, &l.name)?;
        validation.write(value)?;
        let p = Self(unsafe { tp_create() });
        if p.0.is_null() {
            return Err("Publisher allocation failed".into());
        }
        let strings = [
            l.intro,
            l.dispatch,
            l.package,
            l.name,
            l.identifier,
            topic.into(),
        ]
        .map(|s| CString::new(s).map_err(|e| e.to_string()))
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
        let domain = std::env::var("ROS_DOMAIN_ID")
            .unwrap_or_else(|_| "0".into())
            .parse::<usize>()
            .map_err(|e| e.to_string())?;
        p.check(unsafe {
            pp_open(
                p.0,
                strings[0].as_ptr(),
                strings[1].as_ptr(),
                strings[2].as_ptr(),
                strings[3].as_ptr(),
                strings[4].as_ptr(),
                best.into(),
                strings[5].as_ptr(),
                domain,
                transient.into(),
            )
        })?;
        unsafe {
            crate::service_codec::native_write(sp_schema(p.0, 0), sp_message(p.0, 0), value)?;
        }
        Ok(p)
    }
    fn check(&self, code: i32) -> Result<(), String> {
        if code < 0 {
            Err(unsafe { CStr::from_ptr(sp_error(self.0)) }
                .to_string_lossy()
                .into_owned())
        } else {
            Ok(())
        }
    }
    fn matched(&self) -> Result<usize, String> {
        let mut count = 0;
        self.check(unsafe { pp_matched(self.0, &mut count) })?;
        Ok(count)
    }
    fn send(&self) -> Result<(), String> {
        self.check(unsafe { pp_send(self.0) })
    }
}
struct ServiceCall {
    ids: (Generation, RequestId, SubscriptionId),
    client: crate::service_codec::Probe,
    sequence: Option<i64>,
    started: Instant,
    deadline: Instant,
}
struct Statistics {
    ids: (Generation, RequestId, SubscriptionId),
    reader: crate::native_topic::NativeTopic,
    window: Instant,
    count: u64,
    bytes: u64,
}
struct Repeater {
    ids: (Generation, RequestId, SubscriptionId),
    publisher: Publisher,
    period: Duration,
    next: Instant,
    count: u64,
    status_at: Instant,
}
#[derive(Default)]
pub struct ToolsWorker {
    pub(crate) imports:
        std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<String, String>>>,
    generation: Option<Generation>,
    last_request: u64,
    publishers: Vec<Repeater>,
    statistics: Vec<Statistics>,
    calls: Vec<ServiceCall>,
    actions: super::tools_actions::Actions,
    parameters: super::tools_parameters::Parameters,
}
pub(crate) fn event(
    ids: (Generation, RequestId, SubscriptionId),
    state: &str,
    value: Value,
    terminal: bool,
) -> Event {
    Event::Tool {
        generation: ids.0,
        request: ids.1,
        subscription: ids.2,
        update: Update::new(state, value, terminal),
    }
}
impl ToolsWorker {
    pub fn command(&mut self, command: Command, node: &rclrs::Node) -> Vec<Event> {
        let ids = command.ids();
        let Command::Tool { operation, .. } = command else {
            return Vec::new();
        };
        let result = (|| {
            operation.validate()?;
            if self.generation.is_some_and(|g| g != ids.0) || ids.1 .0 <= self.last_request {
                return Err("Stale Tools operation".into());
            }
            self.generation = Some(ids.0);
            self.last_request = ids.1 .0;
            if matches!(
                operation,
                Operation::Call { .. }
                    | Operation::Goal { .. }
                    | Operation::ParameterRead { .. }
                    | Operation::ParameterWrite { .. }
                    | Operation::ParameterEvents { .. }
                    | Operation::Statistics { .. }
                    | Operation::Publish { .. }
            ) && self.calls.len()
                + self.publishers.len()
                + self.statistics.len()
                + self.actions.len()
                + self.parameters.len()
                >= MAX_OPERATIONS
            {
                return Err("Tools operation capacity reached".into());
            }
            match operation {
                Operation::NodeInfo { node: target } => {
                    super::tools_parameters::node_info(ids, node, &target)
                }
                Operation::ParameterRead {
                    node,
                    names,
                    timeout_ms,
                } => self.parameters.read(ids, &node, names, timeout_ms),
                Operation::ParameterWrite {
                    node,
                    changes,
                    atomic,
                    timeout_ms,
                } => self
                    .parameters
                    .write(ids, &node, changes, atomic, timeout_ms),
                Operation::ParameterEvents { node } => self.parameters.events(ids, &node),
                Operation::Graph { section, .. } => {
                    if section == "Nodes" || section == "Parameters" {
                        let mut rows = std::collections::BTreeMap::new();
                        for n in node
                            .get_node_names()
                            .map_err(|e| e.to_string())?
                            .into_iter()
                            .take(studio_protocol::tools::MAX_GRAPH)
                        {
                            rows.insert(
                                format!("{}/{}", n.namespace.trim_end_matches('/'), n.name),
                                vec![if section == "Nodes" {
                                    "node"
                                } else {
                                    "parameters"
                                }],
                            );
                        }
                        return Ok(event(ids, "graph", json!(rows), true));
                    }
                    if !["Topics", "Services", "Actions"].contains(&section.as_str()) {
                        return Err("This section is not implemented yet".into());
                    }
                    let rows = if section == "Services" || section == "Actions" {
                        service_graph(node)
                    } else {
                        node.get_topic_names_and_types()
                    }
                    .map_err(|e| e.to_string())?;
                    let rows = if section == "Actions" {
                        rows.into_iter()
                            .filter_map(|(n, types)| {
                                n.strip_suffix("/_action/send_goal").map(|name| {
                                    (
                                        name.to_owned(),
                                        types
                                            .into_iter()
                                            .filter_map(|t| {
                                                t.strip_suffix("_SendGoal").map(str::to_owned)
                                            })
                                            .collect::<Vec<_>>(),
                                    )
                                })
                            })
                            .collect()
                    } else if section == "Services" {
                        rows.into_iter()
                            .filter(|(name, _)| !name.contains("/_action/"))
                            .collect()
                    } else {
                        rows
                    };
                    if rows.len() > studio_protocol::tools::MAX_GRAPH {
                        return Err("Graph exceeds endpoint budget".into());
                    }
                    Ok(event(ids, "graph", json!(rows), true))
                }
                Operation::Details { topic } => {
                    let describe = |v: Vec<rclrs::TopicEndpointInfo>| {
                        json!(v.into_iter().map(|p|json!({"node":p.node_name,"namespace":p.node_namespace,"type":p.topic_type,"qos":format!("{:?}",p.qos_profile)})).collect::<Vec<_>>())
                    };
                    Ok(event(
                        ids,
                        "details",
                        json!({"publishers":describe(node.get_publishers_info_by_topic(&topic).map_err(|e|e.to_string())?),"subscribers":describe(node.get_subscriptions_info_by_topic(&topic).map_err(|e|e.to_string())?)}),
                        true,
                    ))
                }
                Operation::Schema {
                    message_type,
                    bundle,
                } => {
                    let l = libraries(&message_type, &bundle)?;
                    if message_type.contains("/action/") {
                        return super::tools_actions::schema(ids, &l);
                    }
                    if message_type.contains("/srv/") {
                        let client = crate::service_codec::Probe::load(
                            &l.intro,
                            &l.dispatch,
                            &l.package,
                            &l.name,
                            &l.identifier,
                        )?;
                        return Ok(event(
                            ids,
                            "schema",
                            json!({"fields":client.describe(false)?,"default":client.request_value()?,"response_fields":client.describe(true)?}),
                            true,
                        ));
                    }
                    if !bundle.is_empty() {
                        let imports = self.imports.lock().unwrap();
                        if imports.len() >= 64 && !imports.contains_key(&message_type) {
                            return Err("Imported interface capacity reached".into());
                        }
                    }
                    let m = NativeMessage::load(&l.intro, &l.package, &l.name)?;
                    let fields = m.schema()?;
                    let default = m.read()?;
                    if !bundle.is_empty() {
                        self.imports
                            .lock()
                            .unwrap()
                            .insert(message_type.clone(), bundle.clone());
                    }
                    Ok(event(
                        ids,
                        "schema",
                        json!({"fields":fields,"default":default}),
                        true,
                    ))
                }
                Operation::Publish {
                    topic,
                    message_type,
                    bundle,
                    value,
                    interval_ms,
                    best_effort,
                    transient_local,
                } => {
                    if self.publishers.len() >= MAX_OPERATIONS {
                        return Err("Publisher capacity reached".into());
                    }
                    let publisher = Publisher::open(
                        &topic,
                        &message_type,
                        &bundle,
                        &value,
                        best_effort,
                        transient_local,
                    )?;
                    if interval_ms == 0 {
                        self.publishers.push(Repeater {
                            ids,
                            publisher,
                            period: Duration::ZERO,
                            next: Instant::now() + Duration::from_secs(1),
                            count: 0,
                            status_at: Instant::now(),
                        });
                        Ok(event(
                            ids,
                            "publishing",
                            json!({"count":0,"phase":"Preparing one-shot publisher for discovery"}),
                            false,
                        ))
                    } else {
                        publisher.send()?;
                        let period = Duration::from_millis(interval_ms);
                        self.publishers.push(Repeater {
                            ids,
                            publisher,
                            period,
                            next: Instant::now() + period,
                            count: 1,
                            status_at: Instant::now(),
                        });
                        Ok(event(ids, "publishing", json!({"count":1}), false))
                    }
                }
                Operation::Statistics {
                    topic,
                    message_type,
                    best_effort,
                    transient_local,
                    bundle,
                } => {
                    if self.statistics.len() + self.publishers.len() >= MAX_OPERATIONS {
                        return Err("Operation capacity reached".into());
                    }
                    let mut reader = crate::native_topic::NativeTopic::open_with_bundle(
                        &topic,
                        &message_type,
                        best_effort,
                        transient_local,
                        false,
                        &bundle,
                    )?;
                    reader.enable_message_loss_events()?;
                    self.statistics.push(Statistics {
                        ids,
                        reader,
                        window: Instant::now(),
                        count: 0,
                        bytes: 0,
                    });
                    Ok(event(ids, "statistics started", Value::Null, false))
                }
                Operation::Goal {
                    action,
                    message_type,
                    bundle,
                    value,
                    timeout_ms,
                } => {
                    if self.calls.len()
                        + self.publishers.len()
                        + self.statistics.len()
                        + self.actions.len()
                        >= MAX_OPERATIONS
                    {
                        return Err("Operation capacity reached".into());
                    }
                    self.actions
                        .start(ids, &action, &message_type, &bundle, &value, timeout_ms)
                }
                Operation::Cancel { target } => self.actions.cancel(ids, target),
                Operation::Call {
                    service,
                    message_type,
                    bundle,
                    value,
                    timeout_ms,
                } => {
                    if self.calls.len() + self.publishers.len() + self.statistics.len()
                        >= MAX_OPERATIONS
                    {
                        return Err("Operation capacity reached".into());
                    }
                    let l = libraries(&message_type, &bundle)?;
                    if !message_type.contains("/srv/") {
                        return Err("Expected service type".into());
                    }
                    let mut client = crate::service_codec::Probe::load(
                        &l.intro,
                        &l.dispatch,
                        &l.package,
                        &l.name,
                        &l.identifier,
                    )?;
                    client.request(&value)?;
                    let domain = std::env::var("ROS_DOMAIN_ID")
                        .unwrap_or_else(|_| "0".into())
                        .parse::<usize>()
                        .map_err(|e| e.to_string())?;
                    client.connect_at(&service, domain)?;
                    self.calls.push(ServiceCall {
                        ids,
                        client,
                        sequence: None,
                        started: Instant::now(),
                        deadline: Instant::now() + Duration::from_millis(timeout_ms),
                    });
                    Ok(event(ids, "waiting for service", Value::Null, false))
                }
                Operation::ServiceInfo { service } => {
                    let mut providers = Vec::new();
                    for n in node
                        .get_node_names()
                        .map_err(|e| e.to_string())?
                        .into_iter()
                        .take(4096)
                    {
                        let services = node
                            .get_service_names_and_types_by_node(&n.name, &n.namespace)
                            .map_err(|e| e.to_string())?;
                        if services.contains_key(&service) {
                            providers.push(json!({"name":n.name,"namespace":n.namespace}));
                        }
                    }
                    Ok(event(ids, "details", json!({"providers":providers}), true))
                }
                Operation::Stop { target } => {
                    if let Some(e) = self.parameters.stop(ids, target) {
                        return Ok(e);
                    }
                    if let Some(e) = self.actions.stop(ids, target) {
                        return Ok(e);
                    }
                    if let Some(i) = self.calls.iter().position(|p| p.ids.1 .0 == target) {
                        let call = self.calls.remove(i);
                        return Ok(event(
                            ids,
                            "stopped",
                            json!({"target":target,"request_sent":call.sequence.is_some(),"meaning":"Local wait ended; remote service execution is not cancelled"}),
                            true,
                        ));
                    }
                    if let Some(i) = self.statistics.iter().position(|p| p.ids.1 .0 == target) {
                        self.statistics.remove(i);
                        return Ok(event(ids, "stopped", json!({"target":target}), true));
                    }
                    let Some(index) = self.publishers.iter().position(|p| p.ids.1 .0 == target)
                    else {
                        return Err("No active publisher with this operation ID".into());
                    };
                    let stopped = self.publishers.remove(index);
                    Ok(event(
                        ids,
                        "stopped",
                        json!({"target":target,"publish_calls":stopped.count}),
                        true,
                    ))
                }
            }
        })();
        let mut e = result.unwrap_or_else(|e: String| event(ids, "error", json!(e), true));
        if e.validate().is_err() {
            e = event(
                ids,
                "error",
                json!("Result exceeds Tools payload budget"),
                true,
            );
        }
        vec![e]
    }
    pub fn progress(&mut self) -> Vec<Event> {
        let now = Instant::now();
        let mut events = self.actions.progress();
        events.extend(self.parameters.progress());
        self.publishers.retain_mut(|p| {
            if now < p.next {
                return true;
            }
            if p.period.is_zero() {
                if p.count==0 {match p.publisher.send(){Ok(())=>{p.count=1;p.next=Instant::now()+Duration::from_millis(250);return true;},Err(e)=>{events.push(event(p.ids,"error",json!(e),true));return false;}}}
                events.push(event(p.ids,"published",json!({"count":1,"matching_subscribers":p.publisher.matched().ok(),"scope":"Native publish call; subscriber delivery is not acknowledged by this counter"}),true));return false;
            }
            match p.publisher.send() {
                Ok(()) => {
                    p.count += 1;
                    p.next = Instant::now() + p.period;
                    if p.status_at.elapsed()>=Duration::from_secs(1){events.push(event(p.ids,"publishing",json!({"count":p.count}),false));p.status_at=Instant::now();}
                    true
                }
                Err(e) => {
                    events.push(event(p.ids, "error", json!(e), true));
                    false
                }
            }
        });
        self.calls.retain_mut(|p| {
            let result = (|| -> Result<bool, String> {
                if Instant::now() >= p.deadline {
                    events.push(event(p.ids,"timeout",json!({"request_sent":p.sequence.is_some(),"elapsed_ms":p.started.elapsed().as_millis() as u64,"meaning":"Local wait ended; remote execution is not cancelled"}),true));
                    return Ok(false);
                }
                if p.sequence.is_none() && p.client.available()? {
                    p.sequence=Some(p.client.send()?);
                    events.push(event(p.ids,"request sent",Value::Null,false));
                }
                if let Some(sequence)=p.sequence {
                    if p.client.take(0)? == Some(sequence) {
                        events.push(event(p.ids,"response",json!({"response":p.client.response()?,"elapsed_ms":p.started.elapsed().as_millis() as u64}),true));
                        return Ok(false);
                    }
                }
                Ok(true)
            })();
            match result {Ok(keep)=>keep,Err(e)=>{events.push(event(p.ids,"error",json!(e),true));false}}
        });
        self.statistics.retain_mut(|p|{
            for _ in 0..32 {match p.reader.poll_statistics(){Ok(Some(bytes))=>{p.count+=1;p.bytes+=bytes as u64;},Ok(None)=>break,Err(e)=>{events.push(event(p.ids,"error",json!(e),true));return false;}}}
            let elapsed=p.window.elapsed().as_secs_f64();if elapsed>=1.0{let middleware_loss=match p.reader.message_loss_count(){Ok(value)=>value,Err(error)=>{events.push(event(p.ids,"error",json!(error),true));return false;}};events.push(event(p.ids,"statistics",json!({"window_seconds":elapsed,"received_samples":p.count,"received_hz":p.count as f64/elapsed,"serialized_bytes_per_second":p.bytes as f64/elapsed,"backend_queue_drops":0,"application_queue_drops":0,"middleware_reported_lost":middleware_loss,"scope":"Received by Tools; excludes transport overhead and is not network-wide delivery"}),false));p.window=Instant::now();p.count=0;p.bytes=0;}true
        });
        events
    }
}

fn service_graph(
    node: &rclrs::Node,
) -> Result<std::collections::HashMap<String, Vec<String>>, rclrs::RclrsError> {
    let mut graph = std::collections::HashMap::<String, Vec<String>>::new();
    for n in node
        .get_node_names()?
        .into_iter()
        .take(studio_protocol::tools::MAX_GRAPH)
    {
        for (name, types) in node.get_service_names_and_types_by_node(&n.name, &n.namespace)? {
            let values = graph.entry(name).or_default();
            for typ in types {
                if !values.contains(&typ) {
                    values.push(typ);
                }
            }
        }
    }
    Ok(graph)
}
