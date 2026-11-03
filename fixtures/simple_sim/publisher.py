"""Read-only simulated robot data; deliberately has no command subscriptions."""
import json
import math
import os
import time

import rclpy
from geometry_msgs.msg import TransformStamped
from nav_msgs.msg import Odometry
from rclpy.qos import DurabilityPolicy, QoSProfile, qos_profile_sensor_data
from sensor_msgs.msg import Image, JointState, LaserScan
from std_msgs.msg import String
from tf2_msgs.msg import TFMessage

import model
from snapshot import Snapshot


def transform(stamp, parent, child, x=0.0, y=0.0, z=0.0, angle=0.0, wheel=False):
    item = TransformStamped()
    item.header.stamp, item.header.frame_id, item.child_frame_id = stamp, parent, child
    item.transform.translation.x, item.transform.translation.y, item.transform.translation.z = x, y, z
    if wheel:
        item.transform.rotation.y = math.sin(angle / 2)
    else:
        item.transform.rotation.z = math.sin(angle / 2)
    item.transform.rotation.w = math.cos(angle / 2)
    return item


def camera_message(stamp, frame):
    image = Image()
    image.header.stamp, image.header.frame_id = stamp, frame.frame_id
    image.width, image.height, image.encoding = frame.width, frame.height, frame.encoding
    image.is_bigendian, image.step, image.data = frame.is_bigendian, frame.step, frame.data
    return image


def main():
    if os.environ.get('ROS_DOMAIN_ID') != '187':
        raise SystemExit('This isolated fixture requires ROS_DOMAIN_ID=187')
    rclpy.init()
    node = rclpy.create_node('biggs_data_fixture', namespace='/studio_sim', start_parameter_services=False)
    snapshot = Snapshot()
    retained = QoSProfile(depth=1, durability=DurabilityPolicy.TRANSIENT_LOCAL)
    odom_pub = node.create_publisher(Odometry, 'odometry', 10)
    joint_pub = node.create_publisher(JointState, 'joint_states', 10)
    scan_pub = node.create_publisher(LaserScan, 'scan', qos_profile_sensor_data)
    camera_pub = node.create_publisher(Image, 'camera', qos_profile_sensor_data)
    status_pub = node.create_publisher(String, 'status', 10)
    tf_pub = node.create_publisher(TFMessage, '/tf', 10)
    static_pub = node.create_publisher(TFMessage, '/tf_static', retained)
    description_pub = node.create_publisher(String, 'robot_description', retained)
    snapshot.publish(description_pub, '/studio_sim/robot_description', 'std_msgs/msg/String', String(data=model.description()))
    stamp = node.get_clock().now().to_msg()
    snapshot.publish(static_pub, '/tf_static', 'tf2_msgs/msg/TFMessage', TFMessage(transforms=[
        transform(stamp, 'studio_sim/base_footprint', 'studio_sim/base_link', z=model.WHEEL_RADIUS),
        transform(stamp, 'studio_sim/base_link', 'studio_sim/laser', z=0.0529),
    ]))
    start = time.monotonic()
    count = 0
    camera_count = 0

    def tick():
        nonlocal count, camera_count
        seconds = time.monotonic() - start
        stamp = node.get_clock().now().to_msg()
        x, y, yaw = model.pose(seconds)
        positions, velocities = model.wheels(seconds)
        base = transform(stamp, 'studio_sim/odom', 'studio_sim/base_footprint', x=x, y=y, angle=yaw)
        odom = Odometry()
        odom.header, odom.child_frame_id = base.header, base.child_frame_id
        odom.pose.pose.position.x, odom.pose.pose.position.y = x, y
        odom.pose.pose.orientation = base.transform.rotation
        odom.twist.twist.linear.x = model.PATH_RADIUS * model.TURN_RATE
        odom.twist.twist.angular.z = model.TURN_RATE
        snapshot.publish(odom_pub, '/studio_sim/odometry', 'nav_msgs/msg/Odometry', odom)
        joints = JointState()
        joints.header.stamp, joints.header.frame_id = stamp, 'studio_sim/base_link'
        joints.name, joints.position, joints.velocity = model.JOINTS, positions, velocities
        snapshot.publish(joint_pub, '/studio_sim/joint_states', 'sensor_msgs/msg/JointState', joints)
        wheel_tf = [transform(stamp, 'studio_sim/base_link', 'studio_sim/' + name.removesuffix('_joint'),
                              x=wx, y=wy, angle=angle, wheel=True)
                    for name, wx, wy, angle in zip(model.JOINTS, [model.WHEEL_X, -model.WHEEL_X] * 2,
                                                  [model.WHEEL_Y] * 2 + [-model.WHEEL_Y] * 2, positions)]
        snapshot.publish(tf_pub, '/tf', 'tf2_msgs/msg/TFMessage', TFMessage(transforms=[base, *wheel_tf]))
        if count % 2 == 0:
            laser = LaserScan()
            laser.header.stamp, laser.header.frame_id = stamp, 'studio_sim/laser'
            laser.angle_min, laser.angle_increment = -math.pi, 2 * math.pi / 180
            laser.angle_max = laser.angle_min + 179 * laser.angle_increment
            # Instantaneous synthetic rays, not a rolling physical scanner.
            laser.scan_time, laser.time_increment = 0.1, 0.0
            laser.range_min, laser.range_max = 0.05, 10.0
            laser.ranges = model.scan(x, y, yaw)
            snapshot.publish(scan_pub, '/studio_sim/scan', 'sensor_msgs/msg/LaserScan', laser)
            frame = model.camera(camera_count)
            image = camera_message(stamp, frame)
            snapshot.publish(camera_pub, '/studio_sim/camera', 'sensor_msgs/msg/Image', image)
            camera_count += 1
        if count % 10 == 0:
            status = dict(simulation=True, model='Biggs-inspired kinematic fixture',
                          sample=count, seconds=round(seconds, 2), x=round(x, 3), y=round(y, 3),
                          yaw=round(yaw, 3), motion='predefined circle', command_subscribers=0,
                          camera=dict(frame=camera_count - 1, width=model.CAMERA_WIDTH,
                                      height=model.CAMERA_HEIGHT, encoding=model.CAMERA_ENCODING,
                                      counter='first 8 top-row pixels: uint64 little-endian gray'))
            snapshot.publish(status_pub, '/studio_sim/status', 'std_msgs/msg/String', String(data=json.dumps(status)))
        count += 1

    node.create_timer(0.05, tick)
    print(json.dumps({'event': 'ready', 'domain': 187, 'middleware': os.environ.get('RMW_IMPLEMENTATION'),
                      'model': 'kinematic, no collision dynamics or hardware', 'pid': os.getpid()}), flush=True)
    try:
        rclpy.spin(node)
    except KeyboardInterrupt:
        pass
    finally:
        snapshot.close()
        node.destroy_node()
        if rclpy.ok():
            rclpy.shutdown()


if __name__ == '__main__':
    main()
