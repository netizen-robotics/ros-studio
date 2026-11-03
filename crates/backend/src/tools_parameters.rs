//! Parameter service chains and explicit filtered events. Never writes on restore.
use crate::{
    native_topic::NativeTopic,
    service_codec::Probe,
    tools_worker::{event, libraries},
};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
use studio_protocol::{
    session::Event,
    subscriptions::*,
    tools::{ParameterChange, MAX_OPERATIONS},
};
type Ids = (Generation, RequestId, SubscriptionId);
struct Job {
    ids: Ids,
    node: String,
    steps: VecDeque<(String, Value)>,
    current: Option<(String, Probe, Option<i64>)>,
    data: Value,
    deadline: Instant,
    write_sent: bool,
}
struct Events {
    ids: Ids,
    node: String,
    reader: NativeTopic,
    pending: Option<Value>,
    dropped: u64,
    emit_at: Instant,
}
#[derive(Default)]
pub struct Parameters {
    jobs: Vec<Job>,
    events: Vec<Events>,
}
fn valid_node(node: &str) -> Result<(), String> {
    if !node.starts_with('/') || node.ends_with('/') || node.len() > 256 {
        return Err("Choose a fully qualified node name".into());
    }
    Ok(())
}
impl Parameters {
    pub fn len(&self) -> usize {
        self.jobs.len() + self.events.len()
    }
    pub fn read(
        &mut self,
        ids: Ids,
        node: &str,
        names: Vec<String>,
        timeout: u64,
    ) -> Result<Event, String> {
        valid_node(node)?;
        if self.jobs.len() + self.events.len() >= MAX_OPERATIONS {
            return Err("Parameter operation capacity reached".into());
        }
        let mut steps = VecDeque::new();
        if names.is_empty() {
            steps.push_back(("list_parameters".into(), json!({"prefixes":[],"depth":0})));
        } else {
            steps.push_back(("get_parameters".into(), json!({"names":names})));
            steps.push_back(("describe_parameters".into(), json!({"names":names})));
        }
        self.jobs.push(Job {
            ids,
            node: node.into(),
            steps,
            current: None,
            data: json!({"names":names}),
            deadline: Instant::now() + Duration::from_millis(timeout),
            write_sent: false,
        });
        Ok(event(ids, "reading parameters", Value::Null, false))
    }
    pub fn write(
        &mut self,
        ids: Ids,
        node: &str,
        changes: Vec<ParameterChange>,
        atomic: bool,
        timeout: u64,
    ) -> Result<Event, String> {
        valid_node(node)?;
        if self.jobs.len() + self.events.len() >= MAX_OPERATIONS {
            return Err("Parameter operation capacity reached".into());
        }
        let names: Vec<_> = changes.iter().map(|c| c.name.clone()).collect();
        let values = changes
            .iter()
            .map(ParameterChange::wire)
            .collect::<Result<Vec<_>, _>>()?;
        let steps = VecDeque::from([
            (
                if atomic {
                    "set_parameters_atomically"
                } else {
                    "set_parameters"
                }
                .into(),
                json!({"parameters":values}),
            ),
            ("get_parameters".into(), json!({"names":names})),
            ("describe_parameters".into(), json!({"names":names})),
        ]);
        self.jobs.push(Job {
            ids,
            node: node.into(),
            steps,
            current: None,
            data: json!({"names":names}),
            deadline: Instant::now() + Duration::from_millis(timeout),
            write_sent: false,
        });
        Ok(event(ids, "writing parameters", Value::Null, false))
    }
    pub fn events(&mut self, ids: Ids, node: &str) -> Result<Event, String> {
        valid_node(node)?;
        if self.jobs.len() + self.events.len() >= MAX_OPERATIONS {
            return Err("Parameter operation capacity reached".into());
        }
        let reader = NativeTopic::open(
            "/parameter_events",
            "rcl_interfaces/msg/ParameterEvent",
            false,
            false,
            false,
        )?;
        self.events.push(Events {
            ids,
            node: node.into(),
            reader,
            pending: None,
            dropped: 0,
            emit_at: Instant::now(),
        });
        Ok(event(ids, "parameter events started", Value::Null, false))
    }
    pub fn stop(&mut self, ids: Ids, target: u64) -> Option<Event> {
        if let Some(i) = self.jobs.iter().position(|j| j.ids.1 .0 == target) {
            let job = self.jobs.remove(i);
            return Some(event(
                ids,
                "stopped",
                json!({"target":target,"write_sent":job.write_sent,"meaning":"Local waiting ended; issued parameter writes are not undone"}),
                true,
            ));
        }
        let i = self.events.iter().position(|j| j.ids.1 .0 == target)?;
        self.events.remove(i);
        Some(event(ids, "stopped", json!({"target":target}), true))
    }
    pub fn progress(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        self.jobs.retain_mut(|j|{
        let result=(||->Result<bool,String>{
            if Instant::now()>=j.deadline{out.push(event(j.ids,"parameters timeout",json!({"write_sent":j.write_sent,"completed":j.data,"meaning":"Remote writes may have completed; refresh explicitly"}),true));return Ok(false);}
            if j.current.is_none(){
                let Some((method,request))=j.steps.pop_front()else{out.push(event(j.ids,"parameters",j.data.clone(),true));return Ok(false)};
                let ty=match method.as_str(){"list_parameters"=>"ListParameters","get_parameters"=>"GetParameters","describe_parameters"=>"DescribeParameters","set_parameters"=>"SetParameters","set_parameters_atomically"=>"SetParametersAtomically",_=>return Err("Unknown parameter service".into())};
                let l=libraries(&format!("rcl_interfaces/srv/{ty}"),"")?;let mut client=Probe::load(&l.intro,&l.dispatch,&l.package,&l.name,&l.identifier)?;client.request(&request)?;
                let domain=std::env::var("ROS_DOMAIN_ID").unwrap_or_else(|_|"0".into()).parse::<usize>().map_err(|e|e.to_string())?;client.connect_at(&format!("{}/{}",j.node,method),domain)?;j.current=Some((method,client,None));
            }
            let (method,client,sequence)=j.current.as_mut().unwrap();
            if sequence.is_none()&&client.available()?{*sequence=Some(client.send()?);if method.starts_with("set_"){j.write_sent=true;}}
            if let Some(expected)=sequence{if client.take(0)?==Some(*expected){let result=client.response()?;
                match method.as_str(){
                    "list_parameters"=>{let names=result["result"]["names"].as_array().ok_or("Invalid parameter list")?;if names.len()>256{return Err("More than256 parameters; read a selected list".into());}j.data["names"]=json!(names);j.steps.push_back(("get_parameters".into(),json!({"names":names})));j.steps.push_back(("describe_parameters".into(),json!({"names":names})));}
                    "get_parameters"=>j.data["values"]=result["values"].clone(),
                    "describe_parameters"=>j.data["descriptors"]=result["descriptors"].clone(),
                    _=>{j.data["write_results"]=result.clone();out.push(event(j.ids,"parameter write response",result,false));}
                }j.current=None;
            }}Ok(true)
        })();match result{Ok(keep)=>keep,Err(e)=>{out.push(event(j.ids,"error",json!({"error":e,"write_sent":j.write_sent,"completed":j.data}),true));false}}
    });
        self.events.retain_mut(|e| {
            for _ in 0..16 {
                match e.reader.poll_value() {
                    Ok(Some(v)) => {
                        if serde_json::to_vec(&v)
                            .map(|v| v.len())
                            .unwrap_or(usize::MAX)
                            > 60 * 1024
                        {
                            out.push(event(
                                e.ids,
                                "error",
                                json!("Parameter event exceeds retained budget"),
                                true,
                            ));
                            return false;
                        }
                        if v["node"] == e.node && e.pending.replace(v).is_some() {
                            e.dropped += 1;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        out.push(event(e.ids, "error", json!(error), true));
                        return false;
                    }
                }
            }
            if e.emit_at.elapsed() >= Duration::from_millis(100) {
                if let Some(v) = e.pending.take() {
                    out.push(event(
                        e.ids,
                        "parameter event",
                        json!({"event":v,"coalesced_events":e.dropped}),
                        false,
                    ));
                }
                e.emit_at = Instant::now();
            }
            true
        });
        for e in &mut out {
            if e.validate().is_err() {
                if let Event::Tool { update, .. } = e {
                    *update=studio_protocol::tools::Update::new("error",json!("Parameter result exceeds retained budget; request fewer named parameters"),true);
                }
            }
        }
        out
    }
}
pub(crate) fn node_info(ids: Ids, node: &rclrs::Node, target: &str) -> Result<Event, String> {
    valid_node(target)?;
    let (namespace, name) = target.rsplit_once('/').ok_or("Invalid node name")?;
    let namespace = if namespace.is_empty() { "/" } else { namespace };
    let instances = node
        .get_node_names()
        .map_err(|e| e.to_string())?
        .iter()
        .filter(|n| n.name == name && n.namespace == namespace)
        .count();
    if instances == 0 {
        return Err("Node is no longer discovered".into());
    }
    let publishers = node
        .get_publisher_names_and_types_by_node(name, namespace)
        .map_err(|e| e.to_string())?;
    let subscribers = node
        .get_subscription_names_and_types_by_node(name, namespace)
        .map_err(|e| e.to_string())?;
    let services = node
        .get_service_names_and_types_by_node(name, namespace)
        .map_err(|e| e.to_string())?;
    let clients = node
        .get_client_names_and_types_by_node(name, namespace)
        .map_err(|e| e.to_string())?;
    let actions=|values:&std::collections::HashMap<String,Vec<String>>|->std::collections::BTreeMap<String,Vec<String>>{values.iter().filter_map(|(n,types)|n.strip_suffix("/_action/send_goal").map(|name|(name.into(),types.iter().filter_map(|t|t.strip_suffix("_SendGoal").map(str::to_owned)).collect()))).collect()};
    Ok(event(
        ids,
        "node",
        json!({"instances":instances,"ambiguity":if instances>1{"Duplicate names: graph APIs cannot attribute endpoints to individual processes"}else{""},"publishers":publishers,"subscribers":subscribers,"service_servers":services,"service_clients":clients,"action_servers":actions(&services),"action_clients":actions(&clients)}),
        true,
    ))
}
