//! Goal ownership and cancellation stay inside the backend, with no reconnect replay.
use crate::{
    action_codec::{ActionProbe, Slot},
    tools_worker::{event, libraries, Libraries},
};
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use studio_protocol::{session::Event, subscriptions::*};
type Ids = (Generation, RequestId, SubscriptionId);
#[derive(Default)]
pub struct Actions {
    goals: Vec<Goal>,
}
struct Goal {
    ids: Ids,
    client: ActionProbe,
    uuid: Value,
    goal_sequence: Option<i64>,
    result_sequence: Option<i64>,
    cancel: Option<(Ids, i64)>,
    deadline: Instant,
    accepted: bool,
    feedback: Option<Value>,
    feedback_drops: u64,
    feedback_at: Instant,
    status: Option<i64>,
}
fn client(l: &Libraries) -> Result<ActionProbe, String> {
    ActionProbe::load(
        [&l.intro, &l.dispatch, &l.generic],
        &l.package,
        &l.name,
        &l.identifier,
    )
}
pub(crate) fn schema(ids: Ids, l: &Libraries) -> Result<Event, String> {
    let c = client(l)?;
    let fields = c.describe(Slot::GoalRequest)?;
    let goal = fields
        .as_array()
        .and_then(|f| f.iter().find(|f| f["name"] == "goal"))
        .ok_or("Action goal schema unavailable")?["fields"]
        .clone();
    Ok(event(
        ids,
        "schema",
        json!({"fields":goal,"default":c.read(Slot::GoalRequest)?["goal"],"result_fields":c.describe(Slot::ResultResponse)?,"feedback_fields":c.describe(Slot::Feedback)?}),
        true,
    ))
}
impl Actions {
    pub fn len(&self) -> usize {
        self.goals.len()
    }
    pub fn start(
        &mut self,
        ids: Ids,
        name: &str,
        typ: &str,
        bundle: &str,
        value: &Value,
        timeout: u64,
    ) -> Result<Event, String> {
        if !typ.contains("/action/") {
            return Err("Expected action type".into());
        }
        let l = libraries(typ, bundle)?;
        let mut client = client(&l)?;
        let uuid = json!({"uuid":uuid::Uuid::new_v4().as_bytes().to_vec()});
        client.write(Slot::GoalRequest, &json!({"goal_id":uuid,"goal":value}))?;
        client.write(Slot::ResultRequest, &json!({"goal_id":uuid}))?;
        client.write(
            Slot::CancelRequest,
            &json!({"goal_info":{"goal_id":uuid,"stamp":{"sec":0,"nanosec":0}}}),
        )?;
        let domain = std::env::var("ROS_DOMAIN_ID")
            .unwrap_or_else(|_| "0".into())
            .parse::<usize>()
            .map_err(|e| e.to_string())?;
        client.connect_at(name, domain)?;
        self.goals.push(Goal {
            ids,
            client,
            uuid: uuid.clone(),
            goal_sequence: None,
            result_sequence: None,
            cancel: None,
            deadline: Instant::now() + Duration::from_millis(timeout),
            accepted: false,
            feedback: None,
            feedback_drops: 0,
            feedback_at: Instant::now(),
            status: None,
        });
        Ok(event(
            ids,
            "waiting for action server",
            json!({"goal_id":uuid}),
            false,
        ))
    }
    pub fn cancel(&mut self, ids: Ids, target: u64) -> Result<Event, String> {
        let g = self
            .goals
            .iter_mut()
            .find(|g| g.ids.1 .0 == target)
            .ok_or("No owned goal with this operation ID")?;
        if !g.accepted {
            return Err("Wait for goal acceptance before cancelling".into());
        }
        if g.cancel.is_some() {
            return Err("Cancellation is already pending".into());
        }
        let seq = g.client.send(Slot::CancelRequest)?;
        g.cancel = Some((ids, seq));
        Ok(event(
            ids,
            "cancellation pending",
            json!({"target":target,"goal_id":g.uuid}),
            false,
        ))
    }
    pub fn stop(&mut self, ids: Ids, target: u64) -> Option<Event> {
        let i = self.goals.iter().position(|g| g.ids.1 .0 == target)?;
        let goal = self.goals.remove(i);
        Some(event(
            ids,
            "stopped",
            json!({"target":target,"goal_id":goal.uuid,"meaning":"Tracking ended; no cancellation sent; remote execution may continue"}),
            true,
        ))
    }
    pub fn progress(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        self.goals.retain_mut(|g|{
            let result=(||->Result<bool,String>{
                if !g.accepted&&Instant::now()>=g.deadline{out.push(event(g.ids,"tracking timeout",json!({"goal_id":g.uuid,"goal_sent":g.goal_sequence.is_some(),"meaning":"No automatic cancellation; a sent goal may still execute"}),true));return Ok(false);}
                if g.goal_sequence.is_none()&&g.client.available()?{g.goal_sequence=Some(g.client.send(Slot::GoalRequest)?);out.push(event(g.ids,"goal sent",json!({"goal_id":g.uuid}),false));}
                if g.goal_sequence.is_none(){return Ok(true);}
                let ready=g.client.wait(0)?;
                for (bit,slot) in [(4,Slot::GoalResponse),(16,Slot::ResultResponse),(8,Slot::CancelResponse),(1,Slot::Feedback),(2,Slot::Status)] {
                    if ready&bit==0 {continue;}let Some(seq)=g.client.take(slot)?else{continue};let v=g.client.read(slot)?;if serde_json::to_vec(&v).map_err(|e|e.to_string())?.len()>60*1024{return Err("Action payload exceeds retained budget".into());}
                    match slot {
                        Slot::GoalResponse if Some(seq)==g.goal_sequence&&!g.accepted=>{
                            if v["accepted"]==false{out.push(event(g.ids,"goal rejected",json!({"goal_id":g.uuid}),true));return Ok(false);}
                            if v["accepted"]!=true{return Err("Invalid goal acceptance response".into());}
                            g.accepted=true;g.result_sequence=Some(g.client.send(Slot::ResultRequest)?);out.push(event(g.ids,"goal accepted",json!({"goal_id":g.uuid}),false));
                        }
                        Slot::ResultResponse if Some(seq)==g.result_sequence=>{
                            let status=v["status"].as_i64().ok_or("Missing terminal status")?;let state=match status{4=>"succeeded",5=>"cancelled",6=>"aborted",_=>return Err("Result has a nonterminal action status".into())};
                            if let Some((cancel_ids,_))=g.cancel.take(){out.push(event(cancel_ids,"goal finished before cancellation acknowledgement",json!({"target":g.ids.1.0}),true));}
                            if let Some(feedback)=g.feedback.take(){out.push(event(g.ids,"feedback",json!({"feedback":feedback,"coalesced_feedback":g.feedback_drops}),false));}
                            out.push(event(g.ids,state,json!({"goal_id":g.uuid,"result":v["result"],"coalesced_feedback":g.feedback_drops}),true));return Ok(false);
                        }
                        Slot::CancelResponse=>{
                            if let Some((cancel_ids,expected))=g.cancel{if seq==expected{g.cancel=None;let acknowledged=v["return_code"]==0&&v["goals_canceling"].as_array().is_some_and(|goals|goals.iter().any(|x|x["goal_id"]==g.uuid));out.push(event(cancel_ids,if acknowledged{"cancel acknowledged"}else{"cancel rejected"},json!({"target":g.ids.1.0,"goal_id":g.uuid,"response":v,"meaning":"Cancellation acknowledgement is not terminal completion"}),true));}}
                        }
                        Slot::Feedback if v["goal_id"]==g.uuid=>{if g.feedback.replace(v["feedback"].clone()).is_some(){g.feedback_drops+=1;}}
                        Slot::Status=>{if let Some(items)=v["status_list"].as_array(){if let Some(item)=items.iter().find(|x|x["goal_info"]["goal_id"]==g.uuid){let status=item["status"].as_i64();if status!=g.status{g.status=status;out.push(event(g.ids,"goal status",json!({"goal_id":g.uuid,"status":status}),false));}}}}
                        _=>(),
                    }
                }
                if g.feedback_at.elapsed()>=Duration::from_millis(100){if let Some(feedback)=g.feedback.take(){out.push(event(g.ids,"feedback",json!({"goal_id":g.uuid,"feedback":feedback,"coalesced_feedback":g.feedback_drops}),false));}g.feedback_at=Instant::now();}
                Ok(true)
            })();
            match result{Ok(keep)=>keep,Err(e)=>{out.push(event(g.ids,"tracking lost",json!({"error":e,"goal_id":g.uuid,"meaning":"Remote execution may continue"}),true));false}}
        });
        for e in &mut out {
            if e.validate().is_err() {
                if let Event::Tool { update, .. } = e {
                    *update = studio_protocol::tools::Update::new(
                        "tracking lost",
                        json!(
                            "Action update exceeds payload budget; remote execution may continue"
                        ),
                        true,
                    );
                }
            }
        }
        out
    }
}
