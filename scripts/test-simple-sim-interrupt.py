"""Real localhost ROS regression: second SIGINT during relay cleanup.

Requires the running Docker simulation and the same guarded project environment
as simple-sim-relay.py. The signal is sent only to this test's own process.
"""
import os
from pathlib import Path
import runpy
import signal
import sys

import rclpy.node


def main():
    original = rclpy.node.Node.destroy_node

    def interrupted_cleanup(node):
        os.kill(os.getpid(), signal.SIGINT)
        return original(node)

    rclpy.node.Node.destroy_node = interrupted_cleanup
    try:
        sys.argv = ['scripts/simple-sim-relay.py', '--seconds', '0.5']
        runpy.run_path(str(Path(__file__).with_name('simple-sim-relay.py')), run_name='__main__')
    finally:
        rclpy.node.Node.destroy_node = original


if __name__ == '__main__':
    main()
