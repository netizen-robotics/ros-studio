"""Reusable synthetic action peer; localhost/domain guard belongs to each runner."""
import time
from example_interfaces.action import Fibonacci
from rclpy.action import ActionServer, CancelResponse, GoalResponse
from rclpy.callback_groups import ReentrantCallbackGroup


def create_action(node):
    node.tools_goal_requests = 0
    node.tools_cancel_requests = 0
    def goal(request):
        node.tools_goal_requests += 1
        return GoalResponse.REJECT if request.order < 0 else GoalResponse.ACCEPT
    def execute(handle):
        values = [0, 1]
        if handle.request.order == 99:
            handle.abort()
            return Fibonacci.Result(sequence=values)
        for _ in range(min(handle.request.order, 100)):
            if handle.is_cancel_requested:
                handle.canceled()
                return Fibonacci.Result(sequence=values)
            # Saturate synthetic long-running feedback to the interface int32 bound.
            values.append(min(values[-1] + values[-2], 2**31 - 1))
            handle.publish_feedback(Fibonacci.Feedback(sequence=values))
            time.sleep(0.05)
        handle.succeed()
        return Fibonacci.Result(sequence=values)
    def cancel(handle):
        node.tools_cancel_requests += 1
        return CancelResponse.ACCEPT
    return ActionServer(node, Fibonacci, '/studio_fixture/tools_fibonacci', execute,
                        goal_callback=goal, cancel_callback=cancel,
                        callback_group=ReentrantCallbackGroup())


def create_nav2_action(node):
    """A synthetic action endpoint; no navigation stack or robot is involved."""
    from nav2_msgs.action import NavigateToPose
    node.tools_goal_requests = 0
    node.tools_cancel_requests = 0
    def goal(request):
        node.tools_goal_requests += 1
        return GoalResponse.REJECT if request.pose.pose.position.x < 0 else GoalResponse.ACCEPT
    def execute(handle):
        steps = 100 if handle.request.pose.pose.position.x >= 10 else 3
        for index in range(steps):
            if handle.is_cancel_requested:
                handle.canceled()
                return NavigateToPose.Result()
            feedback = NavigateToPose.Feedback()
            feedback.distance_remaining = float(steps - index)
            handle.publish_feedback(feedback)
            time.sleep(0.05)
        handle.succeed()
        return NavigateToPose.Result()
    def cancel(_handle):
        node.tools_cancel_requests += 1
        return CancelResponse.ACCEPT
    return ActionServer(node, NavigateToPose, '/studio_fixture/tools_nav2', execute,
                        goal_callback=goal, cancel_callback=cancel,
                        callback_group=ReentrantCallbackGroup())


def create_parameters(node):
    from rcl_interfaces.msg import ParameterDescriptor, SetParametersResult, IntegerRange
    node.declare_parameter('enabled', True)
    node.declare_parameter('speed', 2, ParameterDescriptor(description='Synthetic even speed, 0 to 10', integer_range=[IntegerRange(from_value=0, to_value=10, step=2)]))
    node.declare_parameter('gain', 1.25)
    node.declare_parameter('label', 'fixture')
    node.declare_parameter('bytes', [b'\x01', b'\xff'])
    node.declare_parameter('switches', [True, False])
    node.declare_parameter('axes', [1, 2, 3])
    node.declare_parameter('gains', [1.25, 2.5])
    node.declare_parameter('tags', ['one', 'two'])
    node.declare_parameter('serial', 'fixture-only', ParameterDescriptor(read_only=True))
    node.declare_parameter('ephemeral', 123, ParameterDescriptor(dynamic_typing=True))
    def validate(parameters):
        if any(p.name == 'label' and p.value == 'reject' for p in parameters):
            return SetParametersResult(successful=False, reason='Synthetic label rejection')
        return SetParametersResult(successful=True)
    node.add_on_set_parameters_callback(validate)


def create_custom(node):
    from feasibility_interfaces.msg import Sample
    from feasibility_interfaces.srv import Inspect
    from feasibility_interfaces.action import Count
    received = []
    subscription = node.create_subscription(Sample, '/studio_fixture/tools_custom', lambda msg: received.append(msg.sequence), 10)
    def inspect(request, response):
        response.accepted = request.sample.sequence == 18446744073709551615
        response.reason = request.sample.nested.label
        return response
    service = node.create_service(Inspect, '/studio_fixture/tools_inspect', inspect)
    def execute(handle):
        for i in range(min(handle.request.target, 100)):
            if handle.is_cancel_requested:
                handle.canceled()
                return Count.Result(completed=i)
            handle.publish_feedback(Count.Feedback(current=i+1))
            time.sleep(0.02)
        handle.succeed()
        return Count.Result(completed=min(handle.request.target, 100))
    action = ActionServer(node, Count, '/studio_fixture/tools_count', execute,
                         cancel_callback=lambda handle: CancelResponse.ACCEPT,
                         callback_group=ReentrantCallbackGroup())
    return received, [subscription, service, action]
