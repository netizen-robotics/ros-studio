"""Synthetic rclpy action server, only localhost/domain 187."""
import argparse
import json
import os
from pathlib import Path
import time


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--kind',choices=['count','accumulate'],default='count')
    parser.add_argument('--mode',choices=['normal','reject','abort','cancel-reject','hang','delay-goal','flood','foreign'],default='normal')
    parser.add_argument('--log',type=Path,required=True)
    args=parser.parse_args()
    if os.environ.get('ROS_DOMAIN_ID')!='187' or os.environ.get('ROS_LOCALHOST_ONLY')!='1':raise SystemExit('Fixture requires localhost/domain 187')
    import rclpy
    from rclpy.action import ActionServer, CancelResponse, GoalResponse
    from rclpy.callback_groups import ReentrantCallbackGroup
    from rclpy.executors import MultiThreadedExecutor, ExternalShutdownException
    from rosidl_runtime_py.convert import message_to_ordereddict
    if args.kind=='count':
        from feasibility_interfaces.action import Count as Action
    else:
        from service_extra_interfaces.action import Accumulate as Action
    rclpy.init()
    node=rclpy.create_node('studio_action_fixture')
    group=ReentrantCallbackGroup()
    name='/studio_fixture/'+args.kind
    def log(event,**data):
        with args.log.open('a') as f:f.write(json.dumps(dict(event=event,**data))+'\n')
    def goal(request):
        log('goal',goal=message_to_ordereddict(request))
        if args.mode=='delay-goal':time.sleep(8)
        return GoalResponse.REJECT if args.mode=='reject' else GoalResponse.ACCEPT
    def cancel(handle):
        log('cancel',uuid=[int(x) for x in handle.goal_id.uuid])
        return CancelResponse.REJECT if args.mode=='cancel-reject' else CancelResponse.ACCEPT
    foreign=node.create_publisher(Action.Impl.FeedbackMessage,name+'/_action/feedback',10)
    def execute(handle):
        log('execute',uuid=[int(x) for x in handle.goal_id.uuid],goal=message_to_ordereddict(handle.request),monotonic=time.monotonic())
        if args.mode=='hang':time.sleep(10)
        count=handle.request.target if args.kind=='count' else len(handle.request.values)
        completed=0
        for i in range(count):
            if handle.is_cancel_requested:
                handle.canceled()
                break
            feedback=Action.Feedback()
            if args.kind=='count':feedback.current=i+1
            else:feedback.processed=i+1
            handle.publish_feedback(feedback)
            if args.mode=='foreign':
                bad=Action.Impl.FeedbackMessage();bad.goal_id.uuid=[255]*16;bad.feedback=feedback
                foreign.publish(bad)
            completed=i+1
            time.sleep(.001 if args.mode=='flood' else .06)
            if args.mode=='abort' and completed>=2:
                handle.abort();break
        else:
            handle.succeed()
        result=Action.Result()
        if args.kind=='count':result.completed=completed
        else:result.sum=int(sum(handle.request.values[:completed]))
        log('result',status=handle.status,result=message_to_ordereddict(result),uuid=[int(x) for x in handle.goal_id.uuid],monotonic=time.monotonic())
        return result
    server=ActionServer(node,Action,name,execute_callback=execute,goal_callback=goal,cancel_callback=cancel,callback_group=group)
    executor=MultiThreadedExecutor(num_threads=4);executor.add_node(node)
    print('READY',flush=True)
    try:executor.spin()
    except (ExternalShutdownException,KeyboardInterrupt):pass
    finally:
        server.destroy();node.destroy_node();rclpy.try_shutdown()


if __name__=='__main__':main()
