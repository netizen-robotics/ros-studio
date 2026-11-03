// Standard ROS C API boundary. No generated fixture types are linked here.
#include <rcl/rcl.h>
#include <rcl/graph.h>
#include <rcl/error_handling.h>
#include <rcl/event.h>
#include <rmw/events_statuses/message_lost.h>
#include <rmw/qos_profiles.h>
#include <rmw/rmw.h>
#include <rmw/serialized_message.h>
#include <rosidl_runtime_c/string_functions.h>
#include <rosidl_runtime_c/primitives_sequence.h>
#include <rosidl_typesupport_introspection_c/field_types.h>
#include <rosidl_typesupport_introspection_c/service_introspection.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#ifdef _WIN32
#include <windows.h>
typedef void *LIB;
static LIB open_lib(const char *p) { return (void *)LoadLibraryA(p); }
static void *symbol(LIB l,const char *s) { return (void *)GetProcAddress((HMODULE)l,s); }
static void close_lib(LIB l) { if(l) FreeLibrary((HMODULE)l); }
#else
#include <dlfcn.h>
typedef void *LIB;
static LIB open_lib(const char *p) { return dlopen(p,RTLD_NOW|RTLD_LOCAL); }
static void *symbol(LIB l,const char *s) { return dlsym(l,s); }
static void close_lib(LIB l) { if(l) dlclose(l); }
#endif

// Standalone probes reserve stdout for their final JSON document. Vendor
// diagnostics may bypass rcutils, so redirect the descriptor while ROS is alive.
#ifdef _WIN32
#include <io.h>
#include <fcntl.h>
#include <errno.h>
#define studio_dup _dup
#define studio_dup2 _dup2
#define studio_close _close
#else
#include <unistd.h>
#define studio_dup dup
#define studio_dup2 dup2
#define studio_close close
#endif
int sp_redirect_stdout(void) {
  fflush(stdout);
  int saved=studio_dup(1);
  if(saved<0) return -1;
  if(studio_dup2(2,1)<0) {studio_close(saved);return -1;}
#ifdef _WIN32
  // Rust stdout uses the Win32 standard handle, while vendor C stdio uses fd1.
  if(!SetStdHandle(STD_OUTPUT_HANDLE,(HANDLE)_get_osfhandle(1))) {
    int result=studio_dup2(saved,1);(void)result;
    studio_close(saved);return -1;
  }
#endif
  return saved;
}
void sp_restore_stdout(int saved) {
  fflush(stdout);
  int result=studio_dup2(saved,1);(void)result;
#ifdef _WIN32
  SetStdHandle(STD_OUTPUT_HANDLE,(HANDLE)_get_osfhandle(1));
#endif
  studio_close(saved);
}
// Write protocol bytes to the reserved original descriptor, bypassing vendor
// stdout redirection. Keep a single call below the Windows CRT count limit.
int64_t sp_write_reserved_stdout(int saved, const unsigned char *bytes, size_t count) {
  if(count > 1048576) count = 1048576;
#ifdef _WIN32
  // Keep protocol bytes exact: CRT text mode would expand LF into CRLF.
  int mode=_setmode(saved,_O_BINARY);
  if(mode<0) return -1;
  int written=_write(saved, bytes, (unsigned int)count);
  int write_errno=errno;
  DWORD write_error=GetLastError();
  _setmode(saved,mode);
  errno=write_errno;
  SetLastError(write_error);
  return written;
#else
  return write(saved, bytes, count);
#endif
}

typedef rosidl_typesupport_introspection_c__MessageMembers Members;
typedef rosidl_typesupport_introspection_c__MessageMember Member;
typedef struct {
  LIB introspection, dispatch;
  const rosidl_service_type_support_t *support;
  const Members *req_schema, *res_schema;
  void *request, *response;
  rcl_context_t context;
  rcl_node_t node;
  rcl_client_t client;
  rcl_wait_set_t wait;
  int context_ok, node_ok, client_ok, wait_ok;
  char error[2048];
} Probe;
static int failure(Probe *p, const char *stage) {
  snprintf(p->error,sizeof(p->error),"%s: %s",stage,rcl_get_error_string().str);
  rcl_reset_error(); return -1;
}
const char *sp_error(Probe *p) { return p->error; }
void sp_destroy(Probe *p) {
  if(!p) return;
  if(p->wait_ok) { rcl_ret_t r=rcl_wait_set_fini(&p->wait); (void)r; }
  if(p->client_ok) { rcl_ret_t r=rcl_client_fini(&p->client,&p->node); (void)r; }
  if(p->node_ok) { rcl_ret_t r=rcl_node_fini(&p->node); (void)r; }
  if(p->context_ok) {
    rcl_ret_t r=rcl_shutdown(&p->context); (void)r;
    r=rcl_context_fini(&p->context); (void)r;
  }
  if(p->request) { p->req_schema->fini_function(p->request); free(p->request); }
  if(p->response) { p->res_schema->fini_function(p->response); free(p->response); }
  close_lib(p->dispatch); close_lib(p->introspection); free(p);
}
Probe *sp_create(void) { return calloc(1,sizeof(Probe)); }
int sp_load_message(Probe *p,const char *intro_path,const char *package,const char *name) {
  p->introspection=open_lib(intro_path);
  if(!p->introspection) {
    snprintf(p->error,sizeof(p->error),"Cannot load message introspection %s",intro_path);return -1;
  }
  char sym[512];
  snprintf(sym,sizeof(sym),"rosidl_typesupport_introspection_c__get_message_type_support_handle__%s__msg__%s",package,name);
  typedef const rosidl_message_type_support_t *(*Getter)(void);
  Getter getter=(Getter)symbol(p->introspection,sym);
  if(!getter) {snprintf(p->error,sizeof(p->error),"Missing message symbol %s",sym);return -1;}
  const rosidl_message_type_support_t *support=getter();
  if(!support || !support->data) {snprintf(p->error,sizeof(p->error),"Null message metadata for %s",sym);return -1;}
  p->req_schema=support->data;
  p->request=calloc(1,p->req_schema->size_of_);
  if(!p->request) {snprintf(p->error,sizeof(p->error),"Message allocation failed");return -1;}
  p->req_schema->init_function(p->request,ROSIDL_RUNTIME_C_MSG_INIT_ALL);
  return 0;
}
int sp_load(Probe *p,const char *intro_path,const char *dispatch_path,const char *package,const char *name,const char *identifier) {
  char sym[512];
  p->introspection=open_lib(intro_path);
  p->dispatch=open_lib(dispatch_path);
  if(!p->introspection || !p->dispatch) {
    snprintf(p->error,sizeof(p->error),"Cannot load service libraries %s or %s",intro_path,dispatch_path);
#ifndef _WIN32
    const char *e=dlerror(); if(e) strncat(p->error,e,sizeof(p->error)-strlen(p->error)-1);
#endif
    return -1;
  }
  typedef const rosidl_service_type_support_t *(*Getter)(void);
  snprintf(sym,sizeof(sym),"rosidl_typesupport_introspection_c__get_service_type_support_handle__%s__srv__%s",package,name);
  Getter getter=(Getter)symbol(p->introspection,sym);
  if(!getter) { snprintf(p->error,sizeof(p->error),"Missing service symbol %s in %s",sym,intro_path); return -1; }
  const rosidl_service_type_support_t *intro=getter();
  if(!intro || !intro->data) { snprintf(p->error,sizeof(p->error),"Null service introspection for %s",name); return -1; }
  const rosidl_typesupport_introspection_c__ServiceMembers *members=intro->data;
  p->req_schema=members->request_members_; p->res_schema=members->response_members_;
  snprintf(sym,sizeof(sym),"%s__get_service_type_support_handle__%s__srv__%s",identifier,package,name);
  getter=(Getter)symbol(p->dispatch,sym);
  if(!getter) { snprintf(p->error,sizeof(p->error),"Missing service symbol %s in %s",sym,dispatch_path); return -1; }
  p->support=getter();
  if(!p->req_schema || !p->res_schema || !p->support) { snprintf(p->error,sizeof(p->error),"Incomplete service metadata"); return -1; }
  p->request=calloc(1,p->req_schema->size_of_); p->response=calloc(1,p->res_schema->size_of_);
  if(!p->request || !p->response) { free(p->request);free(p->response);p->request=NULL;p->response=NULL;snprintf(p->error,sizeof(p->error),"Allocation failed");return -1; }
  p->req_schema->init_function(p->request,ROSIDL_RUNTIME_C_MSG_INIT_ALL);
  p->res_schema->init_function(p->response,ROSIDL_RUNTIME_C_MSG_INIT_ALL);
  return 0;
}
const Members *sp_schema(Probe *p,int response) { return response?p->res_schema:p->req_schema; }
void *sp_message(Probe *p,int response) { return response?p->response:p->request; }
size_t sp_count(const Members *m) { return m->member_count_; }
const Member *sp_member(const Members *m,size_t i) { return i<m->member_count_ ? &m->members_[i] : NULL; }
const char *sp_name(const Member *m) { return m->name_; }
int sp_type(const Member *m) { return m->type_id_; }
size_t sp_bound(const Member *m) { return m->array_size_; }
size_t sp_string_bound(const Member *m) { return m->string_upper_bound_; }
int sp_array(const Member *m) { return m->is_array_ ? (m->array_size_ && !m->is_upper_bound_ ? 1 : 2) : 0; }
const Members *sp_nested(const Member *m) { return m->members_ ? m->members_->data : NULL; }
void *sp_field(void *message,const Member *m) { return (char *)message+m->offset_; }
size_t sp_length(const Member *m,const void *field) { return m->size_function ? m->size_function(field) : m->array_size_; }
int sp_resize(const Member *m,void *field,size_t n) { return m->resize_function && m->resize_function(field,n); }
void *sp_element(const Member *m,void *field,size_t i) { return m->get_function ? m->get_function(field,i) : NULL; }
// Only this C boundary knows ROS primitive-sequence layout. Metadata/type/count
// checks precede the cast and memcpy; Rust supplies an exactly sized byte slice.
int sp_copy_int8_sequence(const Member *m,const void *field,int8_t *out,size_t count,size_t maximum) {
  if(!m || !field || count>maximum || (count && !out) ||
     m->type_id_!=rosidl_typesupport_introspection_c__ROS_TYPE_INT8 ||
     !m->is_array_ || m->array_size_ || m->is_upper_bound_) return -1;
  const rosidl_runtime_c__int8__Sequence *sequence=field;
  if(sequence->size!=count || sequence->capacity<count || (count && !sequence->data)) return -1;
  if(count) memcpy(out,sequence->data,count);
  return 0;
}
int sp_string_set(void *field,const char *data,size_t n) { return rosidl_runtime_c__String__assignn(field,data,n); }
const char *sp_string_data(const void *field) { return ((const rosidl_runtime_c__String *)field)->data; }
size_t sp_string_size(const void *field) { return ((const rosidl_runtime_c__String *)field)->size; }
int sp_connect_at(Probe *p,const char *service,size_t domain) {
  p->context=rcl_get_zero_initialized_context(); p->node=rcl_get_zero_initialized_node();
  p->client=rcl_get_zero_initialized_client(); p->wait=rcl_get_zero_initialized_wait_set();
  rcl_init_options_t options=rcl_get_zero_initialized_init_options();
  if(rcl_init_options_init(&options,rcl_get_default_allocator())!=RCL_RET_OK) return failure(p,"init options");
  rcl_ret_t result=rcl_init_options_set_domain_id(&options,domain);
  if(result==RCL_RET_OK) result=rcl_init(0,NULL,&options,&p->context);
  rcl_ret_t cleanup=rcl_init_options_fini(&options); (void)cleanup;
  if(result!=RCL_RET_OK) return failure(p,"context init");
  p->context_ok=1;
  rcl_node_options_t node_options=rcl_node_get_default_options(); node_options.enable_rosout=false;
  if(rcl_node_init(&p->node,"studio_service_probe","/studio_fixture",&p->context,&node_options)!=RCL_RET_OK) return failure(p,"node init");
  p->node_ok=1;
  rcl_client_options_t client_options=rcl_client_get_default_options();
  if(rcl_client_init(&p->client,&p->node,p->support,service,&client_options)!=RCL_RET_OK) return failure(p,"client init");
  p->client_ok=1;
  if(rcl_wait_set_init(&p->wait,0,0,0,1,0,0,&p->context,rcl_get_default_allocator())!=RCL_RET_OK) return failure(p,"wait init");
  p->wait_ok=1;return 0;
}
int sp_connect(Probe *p,const char *service) {return sp_connect_at(p,service,187);}
int sp_available(Probe *p) {
  bool ready=false;
  if(rcl_service_server_is_available(&p->node,&p->client,&ready)!=RCL_RET_OK) return failure(p,"availability");
  return ready;
}
int sp_send(Probe *p,int64_t *sequence) {
  if(rcl_send_request(&p->client,p->request,sequence)!=RCL_RET_OK) return failure(p,"send");
  return 0;
}
int sp_take(Probe *p,int64_t timeout_ns,int64_t *sequence) {
  if(rcl_wait_set_clear(&p->wait)!=RCL_RET_OK) return failure(p,"wait clear");
  if(rcl_wait_set_add_client(&p->wait,&p->client,NULL)!=RCL_RET_OK) return failure(p,"wait add");
  rcl_ret_t ret=rcl_wait(&p->wait,timeout_ns);
  if(ret==RCL_RET_TIMEOUT) return 0;
  if(ret!=RCL_RET_OK) return failure(p,"wait");
  rmw_request_id_t header;
  ret=rcl_take_response(&p->client,&header,p->response);
  if(ret==RCL_RET_CLIENT_TAKE_FAILED) { rcl_reset_error(); return 0; }
  if(ret!=RCL_RET_OK) return failure(p,"take response");
  *sequence=header.sequence_number;return 1;
}

// Generic action support. Only ROS-standard cancellation/status types are linked.
#include <rcl_action/rcl_action.h>
#include <rcl_action/graph.h>
#include <rcl_action/wait.h>
#include <action_msgs/srv/detail/cancel_goal__rosidl_typesupport_introspection_c.h>
#include <action_msgs/msg/detail/goal_status_array__rosidl_typesupport_introspection_c.h>

// Dispatch only between verified, standard native handles. Fast DDS asks for
// introspection as well as serialization when registering feedback type objects.
typedef struct {
  const rosidl_message_type_support_t *selected, *intro;
} MessageSupportPair;
static const rosidl_message_type_support_t *paired_support(const rosidl_message_type_support_t *handle,const char *identifier) {
  const MessageSupportPair *pair=handle->data;
  if(strcmp(identifier,pair->selected->typesupport_identifier)==0) return pair->selected;
  if(strcmp(identifier,pair->intro->typesupport_identifier)==0) return pair->intro;
  return NULL;
}
#ifndef STUDIO_ROS_HUMBLE
static const rosidl_type_hash_t *paired_hash(const rosidl_message_type_support_t *handle) {
  const MessageSupportPair *pair=handle->data;
  return pair->intro->get_type_hash_func(pair->intro);
}
static const rosidl_runtime_c__type_description__TypeDescription *paired_description(const rosidl_message_type_support_t *handle) {
  const MessageSupportPair *pair=handle->data;
  return pair->intro->get_type_description_func(pair->intro);
}
static const rosidl_runtime_c__type_description__TypeSource__Sequence *paired_sources(const rosidl_message_type_support_t *handle) {
  const MessageSupportPair *pair=handle->data;
  return pair->intro->get_type_description_sources_func(pair->intro);
}
#endif
typedef struct {
  Probe *base;
  LIB generic;
  rosidl_action_type_support_t support;
  MessageSupportPair feedback_pair;
  rosidl_message_type_support_t feedback_support;
  rcl_action_client_t client;
  int client_ok;
  const Members *schemas[8];
  void *messages[8];
} ActionProbe;
ActionProbe *ap_create(void) {
  ActionProbe *a=calloc(1,sizeof(ActionProbe));
  if(a) {a->base=sp_create();if(!a->base){free(a);return NULL;}}
  return a;
}
const char *ap_error(ActionProbe *a) {return sp_error(a->base);}
void ap_destroy(ActionProbe *a) {
  if(!a) return;
  if(a->base->wait_ok) {rcl_ret_t r=rcl_wait_set_fini(&a->base->wait);(void)r;a->base->wait_ok=0;}
  if(a->client_ok) {rcl_ret_t r=rcl_action_client_fini(&a->client,&a->base->node);(void)r;}
  for(int i=0;i<8;i++) if(a->messages[i]) {a->schemas[i]->fini_function(a->messages[i]);free(a->messages[i]);}
  // All clients/messages are destroyed before their type-support libraries.
  close_lib(a->generic);sp_destroy(a->base);free(a);
}
static const void *ap_get(ActionProbe *a,LIB library,const char *identifier,const char *kind,const char *package,const char *name) {
  char sym[512];snprintf(sym,sizeof(sym),"%s__get_%s_type_support_handle__%s__action__%s",identifier,kind,package,name);
  typedef const void *(*Getter)(void);
  Getter getter=(Getter)symbol(library,sym);
  if(!getter) {snprintf(a->base->error,sizeof(a->base->error),"Missing action symbol %s",sym);return NULL;}
  return getter();
}
int ap_load(ActionProbe *a,const char *intro_path,const char *selected_path,const char *generic_path,const char *package,const char *name,const char *identifier) {
  Probe *p=a->base;
  p->introspection=open_lib(intro_path);p->dispatch=open_lib(selected_path);a->generic=open_lib(generic_path);
  if(!p->introspection || !p->dispatch || !a->generic) {snprintf(p->error,sizeof(p->error),"Cannot load action libraries: %s, %s, %s",intro_path,selected_path,generic_path);return -1;}
  const rosidl_action_type_support_t *generic=ap_get(a,a->generic,"rosidl_typesupport_c","action",package,name);
  if(!generic) return -1;
  a->support=*generic;
  char type[160];
  snprintf(type,sizeof(type),"%s_SendGoal",name);
  a->support.goal_service_type_support=ap_get(a,p->dispatch,identifier,"service",package,type);
  snprintf(type,sizeof(type),"%s_GetResult",name);
  a->support.result_service_type_support=ap_get(a,p->dispatch,identifier,"service",package,type);
  snprintf(type,sizeof(type),"%s_FeedbackMessage",name);
  a->support.feedback_message_type_support=ap_get(a,p->dispatch,identifier,"message",package,type);
  if(!a->support.goal_service_type_support || !a->support.result_service_type_support || !a->support.feedback_message_type_support) return -1;
  a->feedback_pair.selected=a->support.feedback_message_type_support;
  a->feedback_pair.intro=ap_get(a,p->introspection,"rosidl_typesupport_introspection_c","message",package,type);
  if(!a->feedback_pair.intro) return -1;
  a->feedback_support=(rosidl_message_type_support_t){
    "studio_verified_native_pair", &a->feedback_pair, paired_support,
#ifndef STUDIO_ROS_HUMBLE
    paired_hash, paired_description, paired_sources
#endif
  };
  a->support.feedback_message_type_support=&a->feedback_support;
  const char *suffixes[]={"SendGoal_Request","SendGoal_Response","GetResult_Request","GetResult_Response",NULL,NULL,"FeedbackMessage",NULL};
  for(int i=0;i<8;i++) if(suffixes[i]) {
    snprintf(type,sizeof(type),"%s_%s",name,suffixes[i]);
    const rosidl_message_type_support_t *metadata=ap_get(a,p->introspection,"rosidl_typesupport_introspection_c","message",package,type);
    if(!metadata) return -1;
    a->schemas[i]=metadata->data;
  }
  a->schemas[4]=ROSIDL_TYPESUPPORT_INTERFACE__MESSAGE_SYMBOL_NAME(rosidl_typesupport_introspection_c,action_msgs,srv,CancelGoal_Request)()->data;
  a->schemas[5]=ROSIDL_TYPESUPPORT_INTERFACE__MESSAGE_SYMBOL_NAME(rosidl_typesupport_introspection_c,action_msgs,srv,CancelGoal_Response)()->data;
  a->schemas[7]=ROSIDL_TYPESUPPORT_INTERFACE__MESSAGE_SYMBOL_NAME(rosidl_typesupport_introspection_c,action_msgs,msg,GoalStatusArray)()->data;
  for(int i=0;i<8;i++) {
    if(!a->schemas[i]) {snprintf(p->error,sizeof(p->error),"Missing action introspection slot %d",i);return -1;}
    a->messages[i]=calloc(1,a->schemas[i]->size_of_);
    if(!a->messages[i]) {snprintf(p->error,sizeof(p->error),"Action allocation failed");return -1;}
    a->schemas[i]->init_function(a->messages[i],ROSIDL_RUNTIME_C_MSG_INIT_ALL);
  }
  return 0;
}
const Members *ap_schema(ActionProbe *a,int slot) {return slot>=0 && slot<8 ? a->schemas[slot] : NULL;}
void *ap_message(ActionProbe *a,int slot) {return slot>=0 && slot<8 ? a->messages[slot] : NULL;}
int ap_connect_at(ActionProbe *a,const char *name,size_t domain) {
  Probe *p=a->base;
  p->context=rcl_get_zero_initialized_context();p->node=rcl_get_zero_initialized_node();p->wait=rcl_get_zero_initialized_wait_set();
  a->client=rcl_action_get_zero_initialized_client();
  rcl_init_options_t init=rcl_get_zero_initialized_init_options();
  if(rcl_init_options_init(&init,rcl_get_default_allocator())!=RCL_RET_OK) return failure(p,"action init options");
  rcl_ret_t ret=rcl_init_options_set_domain_id(&init,domain);
  if(ret==RCL_RET_OK) ret=rcl_init(0,NULL,&init,&p->context);
  rcl_ret_t cleanup=rcl_init_options_fini(&init);(void)cleanup;
  if(ret!=RCL_RET_OK) return failure(p,"action context init");
  p->context_ok=1;
  rcl_node_options_t node_options=rcl_node_get_default_options();node_options.enable_rosout=false;
  if(rcl_node_init(&p->node,"studio_action_probe","/studio_fixture",&p->context,&node_options)!=RCL_RET_OK) return failure(p,"action node init");
  p->node_ok=1;
  rcl_action_client_options_t options=rcl_action_client_get_default_options();
  if(rcl_action_client_init(&a->client,&p->node,&a->support,name,&options)!=RCL_RET_OK) return failure(p,"action client init");
  a->client_ok=1;
  size_t subs,guards,timers,clients,services;
  if(rcl_action_client_wait_set_get_num_entities(&a->client,&subs,&guards,&timers,&clients,&services)!=RCL_RET_OK) return failure(p,"action wait count");
  if(rcl_wait_set_init(&p->wait,subs,guards,timers,clients,services,0,&p->context,rcl_get_default_allocator())!=RCL_RET_OK) return failure(p,"action wait init");
  p->wait_ok=1;return 0;
}
int ap_connect(ActionProbe *a,const char *name) {return ap_connect_at(a,name,187);}
int ap_available(ActionProbe *a) {
  bool ready=false;
  if(rcl_action_server_is_available(&a->base->node,&a->client,&ready)!=RCL_RET_OK) return failure(a->base,"action availability");
  return ready;
}
int ap_send(ActionProbe *a,int slot,int64_t *sequence) {
  rcl_ret_t ret;
  if(slot==0) ret=rcl_action_send_goal_request(&a->client,a->messages[0],sequence);
  else if(slot==2) ret=rcl_action_send_result_request(&a->client,a->messages[2],sequence);
  else if(slot==4) ret=rcl_action_send_cancel_request(&a->client,a->messages[4],sequence);
  else {snprintf(a->base->error,sizeof(a->base->error),"Invalid send slot");return -1;}
  if(ret!=RCL_RET_OK) return failure(a->base,"action send");
  return 0;
}
int ap_wait(ActionProbe *a,int64_t timeout_ns) {
  Probe *p=a->base;
  if(rcl_wait_set_clear(&p->wait)!=RCL_RET_OK) return failure(p,"action wait clear");
  if(rcl_action_wait_set_add_action_client(&p->wait,&a->client,NULL,NULL)!=RCL_RET_OK) return failure(p,"action wait add");
  rcl_ret_t ret=rcl_wait(&p->wait,timeout_ns);
  if(ret==RCL_RET_TIMEOUT)return 0;
  if(ret!=RCL_RET_OK)return failure(p,"action wait");
  bool feedback,status,goal,cancel,result;
  if(rcl_action_client_wait_set_get_entities_ready(&p->wait,&a->client,&feedback,&status,&goal,&cancel,&result)!=RCL_RET_OK) return failure(p,"action ready");
  return (feedback?1:0)|(status?2:0)|(goal?4:0)|(cancel?8:0)|(result?16:0);
}
int ap_take(ActionProbe *a,int slot,int64_t *sequence) {
  rmw_request_id_t header={0};rcl_ret_t ret;
  switch(slot) {
    case 1:ret=rcl_action_take_goal_response(&a->client,&header,a->messages[1]);break;
    case 3:ret=rcl_action_take_result_response(&a->client,&header,a->messages[3]);break;
    case 5:ret=rcl_action_take_cancel_response(&a->client,&header,a->messages[5]);break;
    case 6:ret=rcl_action_take_feedback(&a->client,a->messages[6]);break;
    case 7:ret=rcl_action_take_status(&a->client,a->messages[7]);break;
    default:snprintf(a->base->error,sizeof(a->base->error),"Invalid take slot");return -1;
  }
  if(ret==RCL_RET_ACTION_CLIENT_TAKE_FAILED || ret==RCL_RET_CLIENT_TAKE_FAILED || ret==RCL_RET_SUBSCRIPTION_TAKE_FAILED) {rcl_reset_error();return 0;}
  if(ret!=RCL_RET_OK) return failure(a->base,"action take");
  *sequence=header.sequence_number;return 1;
}
// Fixture-only generic native topic ownership. No generated sensor types.
typedef struct {
  Probe base;
  MessageSupportPair support_pair;
  rosidl_message_type_support_t support;
  rcl_subscription_t subscription;
  rcl_publisher_t publisher;
  rcl_event_t lost;
  rmw_message_info_t last_info;
  int subscription_ok, publisher_ok, lost_ok, sample_ok;
} TopicProbe;
void *tp_create(void) { return calloc(1,sizeof(TopicProbe)); }
void tp_destroy(TopicProbe *t) {
  if(!t) return;
  if(t->base.wait_ok) {
    rcl_ret_t r=rcl_wait_set_fini(&t->base.wait);(void)r;
    t->base.wait_ok=0;
  }
  if(t->publisher_ok) {rcl_ret_t r=rcl_publisher_fini(&t->publisher,&t->base.node);(void)r;}
  if(t->lost_ok) {rcl_ret_t r=rcl_event_fini(&t->lost);(void)r;}
  if(t->subscription_ok) {rcl_ret_t r=rcl_subscription_fini(&t->subscription,&t->base.node);(void)r;}
  sp_destroy(&t->base);
}
static int tp_open_impl(TopicProbe *t,const char *intro,const char *dispatch,const char *package,const char *name,const char *identifier,int best_effort,const char *topic,size_t domain,int transient,int echo,int publish_only) {
  Probe *p=&t->base;
  if(echo && (strcmp(topic,"/studio_fixture") || domain!=187 || !getenv("ROS_DOMAIN_ID") || strcmp(getenv("ROS_DOMAIN_ID"),"187") ||
     !getenv("ROS_LOCALHOST_ONLY") || strcmp(getenv("ROS_LOCALHOST_ONLY"),"1"))) {
    snprintf(p->error,sizeof(p->error),"Native topic probe requires domain187 and localhost");return -1;
  }
  if(sp_load_message(p,intro,package,name)<0) return -1;
  p->dispatch=open_lib(dispatch);
  char sym[512];
  snprintf(sym,sizeof(sym),"%s__get_message_type_support_handle__%s__msg__%s",identifier,package,name);
  typedef const rosidl_message_type_support_t *(*Getter)(void);
  Getter getter=p->dispatch ? (Getter)symbol(p->dispatch,sym) : NULL;
  if(!getter) {snprintf(p->error,sizeof(p->error),"Missing topic support %s in %s",sym,dispatch);return -1;}
  const rosidl_message_type_support_t *support=getter();
  if(!support) {snprintf(p->error,sizeof(p->error),"Null topic support");return -1;}
  snprintf(sym,sizeof(sym),"rosidl_typesupport_introspection_c__get_message_type_support_handle__%s__msg__%s",package,name);
  Getter intro_getter=(Getter)symbol(p->introspection,sym);
  if(!intro_getter) {snprintf(p->error,sizeof(p->error),"Missing topic introspection %s",sym);return -1;}
  t->support_pair=(MessageSupportPair){support,intro_getter()};
  t->support=(rosidl_message_type_support_t){
    "studio_verified_native_pair", &t->support_pair, paired_support,
#ifndef STUDIO_ROS_HUMBLE
    paired_hash, paired_description, paired_sources
#endif
  };
  support=&t->support;
  p->context=rcl_get_zero_initialized_context();p->node=rcl_get_zero_initialized_node();
  p->wait=rcl_get_zero_initialized_wait_set();
  t->subscription=rcl_get_zero_initialized_subscription();t->publisher=rcl_get_zero_initialized_publisher();
  rcl_init_options_t init=rcl_get_zero_initialized_init_options();
  if(rcl_init_options_init(&init,rcl_get_default_allocator())!=RCL_RET_OK) return failure(p,"topic init options");
  rcl_ret_t ret=rcl_init_options_set_domain_id(&init,domain);
  if(ret==RCL_RET_OK) ret=rcl_init(0,NULL,&init,&p->context);
  rcl_ret_t cleanup=rcl_init_options_fini(&init);(void)cleanup;
  if(ret!=RCL_RET_OK) return failure(p,"topic context");p->context_ok=1;
  rcl_node_options_t options=rcl_node_get_default_options();options.enable_rosout=false;
  if(rcl_node_init(&p->node,"studio_native_topic_probe","/studio_fixture",&p->context,&options)!=RCL_RET_OK) return failure(p,"topic node");p->node_ok=1;
  rcl_subscription_options_t sub=rcl_subscription_get_default_options();
  // Parameter changes need the standard event history, not the sensor queue.
  if(strcmp(package,"rcl_interfaces")==0 && strcmp(name,"ParameterEvent")==0)
    sub.qos=rmw_qos_profile_parameter_events;
  else sub.qos.depth=2;
  if(transient) sub.qos.durability=RMW_QOS_POLICY_DURABILITY_TRANSIENT_LOCAL;
  sub.qos.reliability=best_effort ? RMW_QOS_POLICY_RELIABILITY_BEST_EFFORT : RMW_QOS_POLICY_RELIABILITY_RELIABLE;
  if(!publish_only) {
  if(rcl_subscription_init(&t->subscription,&p->node,support,topic,&sub)!=RCL_RET_OK) return failure(p,"topic subscription");t->subscription_ok=1;
  }
  if(echo || publish_only) {
  rcl_publisher_options_t pub=rcl_publisher_get_default_options();pub.qos.depth=2;
  pub.qos.reliability=best_effort ? RMW_QOS_POLICY_RELIABILITY_BEST_EFFORT : RMW_QOS_POLICY_RELIABILITY_RELIABLE;
  if(transient) pub.qos.durability=RMW_QOS_POLICY_DURABILITY_TRANSIENT_LOCAL;
  if(rcl_publisher_init(&t->publisher,&p->node,support,publish_only ? topic : "/studio_echo",&pub)!=RCL_RET_OK) return failure(p,"topic publisher");t->publisher_ok=1;
  }
  if(publish_only) return 0;
  if(rcl_wait_set_init(&p->wait,1,0,0,0,0,0,&p->context,rcl_get_default_allocator())!=RCL_RET_OK) return failure(p,"topic wait init");p->wait_ok=1;
  return 0;
}
int tp_open(TopicProbe *t,const char *i,const char *d,const char *p,const char *n,const char *id,int best,const char *topic,size_t domain,int transient,int echo) {
 return tp_open_impl(t,i,d,p,n,id,best,topic,domain,transient,echo,0);
}
int pp_open(TopicProbe *t,const char *i,const char *d,const char *p,const char *n,const char *id,int best,const char *topic,size_t domain,int transient) {
 return tp_open_impl(t,i,d,p,n,id,best,topic,domain,transient,0,1);
}
int pp_send(TopicProbe *t) {
 if(!t->publisher_ok) return failure(&t->base,"Publisher is not open");
 if(rcl_publish(&t->publisher,t->base.request,NULL)!=RCL_RET_OK) return failure(&t->base,"publish");
 return 0;
}
// Opt-in: legacy readers retain their existing event/resource behavior.
int tp_enable_loss(TopicProbe *t) {
  if(t->lost_ok) return 1;
  t->lost=rcl_get_zero_initialized_event();
  rcl_ret_t ret=rcl_subscription_event_init(&t->lost,&t->subscription,RCL_SUBSCRIPTION_MESSAGE_LOST);
  if(ret==RCL_RET_UNSUPPORTED) {rcl_reset_error();return 0;}
  if(ret!=RCL_RET_OK) return failure(&t->base,"topic message-lost event");
  t->lost_ok=1;return 1;
}
int tp_loss_count(TopicProbe *t,uint64_t *total) {
  if(!t->lost_ok) return 0;
  rmw_message_lost_status_t status={0};
  rcl_ret_t ret=rcl_take_event(&t->lost,&status);
  if(ret==RCL_RET_EVENT_TAKE_FAILED) {rcl_reset_error();return 0;}
  if(ret!=RCL_RET_OK) return failure(&t->base,"topic message-lost take");
  *total=(uint64_t)status.total_count;return 1;
}
// Export bounded scalars/text only; Rust never depends on rmw struct layout.
int tp_publication(TopicProbe *t,char *publisher,size_t capacity,uint64_t *sequence) {
  if(!t->sample_ok || !publisher || !sequence || capacity<2*RMW_GID_STORAGE_SIZE+1) {
    snprintf(t->base.error,sizeof(t->base.error),"No valid native publication metadata/storage");return -1;
  }
  static const char hex[]="0123456789abcdef";
  for(size_t i=0;i<RMW_GID_STORAGE_SIZE;i++) {
    uint8_t byte=t->last_info.publisher_gid.data[i];
    publisher[2*i]=hex[byte>>4];publisher[2*i+1]=hex[byte&15];
  }
  publisher[2*RMW_GID_STORAGE_SIZE]='\0';
  *sequence=t->last_info.publication_sequence_number;
  return *sequence==RMW_MESSAGE_INFO_SEQUENCE_NUMBER_UNSUPPORTED?0:1;
}
int tp_connect(TopicProbe *t,const char *intro,const char *dispatch,const char *package,const char *name,const char *identifier,int best_effort) {
  return tp_open(t,intro,dispatch,package,name,identifier,best_effort,"/studio_fixture",187,0,1);
}
int tp_echo(TopicProbe *t,int64_t timeout_ns) {
  Probe *p=&t->base;
  if(rcl_wait_set_clear(&p->wait)!=RCL_RET_OK) return failure(p,"topic wait clear");
  if(rcl_wait_set_add_subscription(&p->wait,&t->subscription,NULL)!=RCL_RET_OK) return failure(p,"topic wait add");
  rcl_ret_t ret=rcl_wait(&p->wait,timeout_ns);
  if(ret==RCL_RET_TIMEOUT)return 0;
  if(ret!=RCL_RET_OK)return failure(p,"topic wait");
  // Each take owns fresh initialized C storage. Some supported RMW C sequence
  // deserializers initialize rather than resize existing destination buffers.
  // Previous fields have already been copied by the owning Rust worker.
  t->sample_ok=0;
  p->req_schema->fini_function(p->request);
  memset(p->request,0,p->req_schema->size_of_);
  p->req_schema->init_function(p->request,ROSIDL_RUNTIME_C_MSG_INIT_ALL);
  rmw_message_info_t info={0};
  ret=rcl_take(&t->subscription,p->request,&info,NULL);
  if(ret==RCL_RET_SUBSCRIPTION_TAKE_FAILED){rcl_reset_error();return 0;}
  if(ret!=RCL_RET_OK)return failure(p,"topic take");
  t->last_info=info;t->sample_ok=1;
  if(t->publisher_ok && rcl_publish(&t->publisher,p->request,NULL)!=RCL_RET_OK)return failure(p,"topic echo");
  return 1;
}

int tp_serialized_size(TopicProbe *t,size_t *size) {
 rmw_serialized_message_t data=rmw_get_zero_initialized_serialized_message();
 rcutils_allocator_t allocator=rcutils_get_default_allocator();
 if(rmw_serialized_message_init(&data,0,&allocator)!=RMW_RET_OK) return failure(&t->base,"serialization allocation");
 rmw_ret_t ret=rmw_serialize(t->base.request,&t->support,&data);
 *size=data.buffer_length;
 rmw_ret_t cleanup=rmw_serialized_message_fini(&data);(void)cleanup;
 if(ret!=RMW_RET_OK) return failure(&t->base,"serialize");
 return 0;
}

int pp_matched(TopicProbe *t,size_t *count) {
  if(!t->publisher_ok) return failure(&t->base,"Publisher is not open");
  if(rcl_publisher_get_subscription_count(&t->publisher,count)!=RCL_RET_OK) return failure(&t->base,"publisher matches");
  return 0;
}
