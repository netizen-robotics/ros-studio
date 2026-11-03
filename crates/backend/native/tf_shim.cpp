// Narrow ABI boundary to the standard ROS tf2 buffer. No transform math is reimplemented.
#include <tf2/buffer_core.hpp>
#include <geometry_msgs/msg/transform_stamped.hpp>
#include <cstdio>
#include <exception>
#include <new>
#include <map>
#include <string>
struct StudioTf {
  tf2::BufferCore buffer;
  // Keep the latest accepted static transform per child across clock epochs.
  std::map<std::string, geometry_msgs::msg::TransformStamped> statics;
  char error[1024] = {};
};
extern "C" {
void *stf_create() { try { return new StudioTf(); } catch (...) { return nullptr; } }
void stf_destroy(void *p) { delete static_cast<StudioTf *>(p); }
const char *stf_error(void *p) { return static_cast<StudioTf *>(p)->error; }
int stf_clear(void *p) {
  auto *t=static_cast<StudioTf *>(p);
  try {
    t->buffer.clear();
    for (const auto &entry : t->statics) {
      if (!t->buffer.setTransform(entry.second,"studio_fixture",true)) {
        std::snprintf(t->error,sizeof(t->error),"tf2 rejected retained static transform");
        return -1;
      }
    }
    return 0;
  }
  catch(const std::exception &e){std::snprintf(t->error,sizeof(t->error),"%s",e.what());return -1;}
}
int stf_set(void *p,const char *parent,const char *child,int64_t stamp,const double *v,int is_static) {
  auto *t=static_cast<StudioTf *>(p);
  try {
    geometry_msgs::msg::TransformStamped msg;
    msg.header.frame_id=parent;msg.child_frame_id=child;
    msg.header.stamp.sec=static_cast<int32_t>(stamp/1000000000);
    msg.header.stamp.nanosec=static_cast<uint32_t>(stamp%1000000000);
    msg.transform.translation.x=v[0];msg.transform.translation.y=v[1];msg.transform.translation.z=v[2];
    msg.transform.rotation.x=v[3];msg.transform.rotation.y=v[4];msg.transform.rotation.z=v[5];msg.transform.rotation.w=v[6];
    if(!t->buffer.setTransform(msg,"studio_fixture",is_static!=0)) {
      std::snprintf(t->error,sizeof(t->error),"tf2 rejected transform %s -> %s",parent,child);return -1;
    }
    if(is_static) t->statics[msg.child_frame_id]=msg;
    else t->statics.erase(msg.child_frame_id);
    return 0;
  }catch(const std::exception &e){std::snprintf(t->error,sizeof(t->error),"%s",e.what());return -1;}
}
int stf_lookup(void *p,const char *target,const char *source,int64_t stamp,double *v) {
  auto *t=static_cast<StudioTf *>(p);
  try {
    auto msg=t->buffer.lookupTransform(target,source,tf2::TimePoint(std::chrono::nanoseconds(stamp)));
    v[0]=msg.transform.translation.x;v[1]=msg.transform.translation.y;v[2]=msg.transform.translation.z;
    v[3]=msg.transform.rotation.x;v[4]=msg.transform.rotation.y;v[5]=msg.transform.rotation.z;v[6]=msg.transform.rotation.w;
    return 0;
  }catch(const std::exception &e){std::snprintf(t->error,sizeof(t->error),"%s",e.what());return -1;}
}
// Robot presentation only: retain native latest-common-time semantics and the
// resolved stamp, including zero for static-only/unknown-frame identity results.
// Acquisition-time scan lookups continue to use the unchanged stf_lookup above.
int stf_lookup_latest(void *p,const char *target,const char *source,double *v,int64_t *resolved_stamp) {
  auto *t=static_cast<StudioTf *>(p);
  try {
    auto msg=t->buffer.lookupTransform(target,source,tf2::TimePoint(std::chrono::nanoseconds(0)));
    v[0]=msg.transform.translation.x;v[1]=msg.transform.translation.y;v[2]=msg.transform.translation.z;
    v[3]=msg.transform.rotation.x;v[4]=msg.transform.rotation.y;v[5]=msg.transform.rotation.z;v[6]=msg.transform.rotation.w;
    *resolved_stamp=static_cast<int64_t>(msg.header.stamp.sec)*1000000000+msg.header.stamp.nanosec;
    return 0;
  }catch(const std::exception &e){std::snprintf(t->error,sizeof(t->error),"%s",e.what());return -1;}
}
}
