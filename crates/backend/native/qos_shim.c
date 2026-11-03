// Standard rcl subscription/event boundary for APIs not exposed by rclrs.
#include <rcl/rcl.h>
#include <rcl/event.h>
#include <rcl/error_handling.h>
#include <rmw/events_statuses/requested_deadline_missed.h>
#include <rmw/events_statuses/liveliness_changed.h>
#include <std_msgs/msg/int32.h>
#include <std_msgs/msg/detail/int32__type_support.h>
#include <stdlib.h>
#include <stdio.h>
typedef struct {
 rcl_context_t context; rcl_node_t node; rcl_subscription_t sub; rcl_event_t deadline,live;
 int context_ok,node_ok,sub_ok,deadline_ok,live_ok; char error[1024];
} QosProbe;
static int qfail(QosProbe*p,const char*stage){snprintf(p->error,sizeof(p->error),"%s: %s",stage,rcl_get_error_string().str);rcl_reset_error();return -1;}
void *sq_create(void){return calloc(1,sizeof(QosProbe));}
const char *sq_error(QosProbe*p){return p->error;}
void sq_destroy(QosProbe*p){
 if(!p)return;
 rcl_ret_t ret;
 if(p->live_ok){ret=rcl_event_fini(&p->live);(void)ret;}
 if(p->deadline_ok){ret=rcl_event_fini(&p->deadline);(void)ret;}
 if(p->sub_ok){ret=rcl_subscription_fini(&p->sub,&p->node);(void)ret;}
 if(p->node_ok){ret=rcl_node_fini(&p->node);(void)ret;}
 if(p->context_ok){ret=rcl_shutdown(&p->context);(void)ret;ret=rcl_context_fini(&p->context);(void)ret;}
 free(p);
}
int sq_init(QosProbe*p,int history){
 p->context=rcl_get_zero_initialized_context();p->node=rcl_get_zero_initialized_node();p->sub=rcl_get_zero_initialized_subscription();
 p->deadline=rcl_get_zero_initialized_event();p->live=rcl_get_zero_initialized_event();
 rcl_init_options_t init=rcl_get_zero_initialized_init_options();
 if(rcl_init_options_init(&init,rcl_get_default_allocator())!=RCL_RET_OK)return qfail(p,"init options");
 rcl_ret_t ret=rcl_init_options_set_domain_id(&init,187);
 if(ret==RCL_RET_OK)ret=rcl_init(0,NULL,&init,&p->context);
 rcl_ret_t cleanup=rcl_init_options_fini(&init);(void)cleanup;
 if(ret!=RCL_RET_OK)return qfail(p,"context");p->context_ok=1;
 rcl_node_options_t no=rcl_node_get_default_options();no.enable_rosout=false;
 if(rcl_node_init(&p->node,"studio_qos_probe","/studio_fixture",&p->context,&no)!=RCL_RET_OK)return qfail(p,"node");p->node_ok=1;
 rcl_subscription_options_t options=rcl_subscription_get_default_options();options.qos.depth=history?3:10;
 if(!history){options.qos.deadline=(rmw_time_t){0,200000000};options.qos.liveliness=RMW_QOS_POLICY_LIVELINESS_MANUAL_BY_TOPIC;options.qos.liveliness_lease_duration=(rmw_time_t){0,300000000};}
 const rosidl_message_type_support_t *support=ROSIDL_GET_MSG_TYPE_SUPPORT(std_msgs,msg,Int32);
 if(rcl_subscription_init(&p->sub,&p->node,support,"/studio_fixture/qos",&options)!=RCL_RET_OK)return qfail(p,"subscription");p->sub_ok=1;
 if(!history){
  if(rcl_subscription_event_init(&p->deadline,&p->sub,RCL_SUBSCRIPTION_REQUESTED_DEADLINE_MISSED)!=RCL_RET_OK)return qfail(p,"deadline event unsupported");p->deadline_ok=1;
  if(rcl_subscription_event_init(&p->live,&p->sub,RCL_SUBSCRIPTION_LIVELINESS_CHANGED)!=RCL_RET_OK)return qfail(p,"liveliness event unsupported");p->live_ok=1;
 }
 return 0;
}
int sq_take(QosProbe*p,int32_t*value){
 std_msgs__msg__Int32 msg={0};rmw_message_info_t info;
 rcl_ret_t ret=rcl_take(&p->sub,&msg,&info,NULL);
 if(ret==RCL_RET_SUBSCRIPTION_TAKE_FAILED){rcl_reset_error();return 0;}
 if(ret!=RCL_RET_OK)return qfail(p,"take");*value=msg.data;return 1;
}
int sq_deadline(QosProbe*p,int32_t*total){
 rmw_requested_deadline_missed_status_t status={0};rcl_ret_t ret=rcl_take_event(&p->deadline,&status);
 if(ret==RCL_RET_EVENT_TAKE_FAILED){rcl_reset_error();return 0;}
 if(ret!=RCL_RET_OK)return qfail(p,"deadline take");*total=status.total_count;return 1;
}
int sq_liveliness(QosProbe*p,int32_t*alive,int32_t*not_alive){
 rmw_liveliness_changed_status_t status={0};rcl_ret_t ret=rcl_take_event(&p->live,&status);
 if(ret==RCL_RET_EVENT_TAKE_FAILED){rcl_reset_error();return 0;}
 if(ret!=RCL_RET_OK)return qfail(p,"liveliness take");*alive=status.alive_count;*not_alive=status.not_alive_count;return 1;
}
