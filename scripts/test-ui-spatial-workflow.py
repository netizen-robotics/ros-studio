"""Offline synthetic Spatial geometry, pressure bounds and guard checks."""
import contextlib
import importlib.util
import io
import math
from pathlib import Path
import queue
import threading
import unittest
from unittest.mock import patch

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('spatial_acceptance',ROOT/'scripts/test-spatial-session.py')
acceptance=importlib.util.module_from_spec(spec);spec.loader.exec_module(acceptance)
fixture=acceptance.FIXTURE


def obj(fields):
    return {'kind':'object','original_fields':len(fields),'fields':[{'name':k,'value':v} for k,v in fields]}
def f(value):return {'kind':'float','value':{'kind':'finite','value':str(value)}}
def envelope(elapsed,same_frame=False):
    scan=fixture.scan_values(elapsed,same_frame)
    frame=scan.pop('frame')
    fields=[('header',obj([('frame_id',{'kind':'text','value':frame,'original_bytes':len(frame)}),
        ('stamp',obj([('sec',{'kind':'signed','value':'2'}),('nanosec',{'kind':'unsigned','value':'3'})]))]))]
    for key,value in scan.items():
        fields.append((key,{'kind':'array','original_items':len(value),'items':[f(v) for v in value]} if isinstance(value,list) else f(value)))
    x,y,yaw=(0.,0.,0.) if same_frame else fixture.MODEL.pose(elapsed)
    return {'sample':{'contract_version':1,'fixed_frame':fixture.FIXED,'source_frame':frame,'acquisition_stamp_ns':2000000003,
        'scan':{'contract_version':2,'message_type':'sensor_msgs/msg/LaserScan','topic':fixture.PREFIX+'scan','source_time':None,'value':obj(fields)},
        'tf':{'status':'ready','components':[x,y,0. if same_frame else .0529,0.,0.,math.sin(yaw/2),math.cos(yaw/2)]}}}


class SpatialFixtureTests(unittest.TestCase):
    def test_known_moving_room_and_same_frame_have_independent_wall_alignment(self):
        for elapsed in [0.,1.,3.,10.]:
            result=acceptance.validate_projection(envelope(elapsed))
            self.assertLess(result['maximum_wall_error_metres'],1e-10)
        acceptance.validate_projection(envelope(8.,True),same_frame=True)
        wrong=envelope(3.);wrong['sample']['tf']['components'][0]+=1
        with self.assertRaises(AssertionError):acceptance.validate_projection(wrong)
        wrong=envelope(3.);wrong['sample']['acquisition_stamp_ns']+=1
        with self.assertRaises(AssertionError):acceptance.validate_projection(wrong)

    def test_scheduling_skips_history_and_delayed_tf_retention_has_hard_eight_item_bound(self):
        schedule=fixture.Schedule(0.)
        self.assertEqual(set(schedule.due(0.)),set(schedule.periods))
        self.assertEqual(schedule.due(0.),[])
        self.assertEqual(schedule.due(.051),['tf'])
        self.assertEqual(set(schedule.due(1000.)),set(schedule.periods))
        self.assertEqual(schedule.due(1000.),[])
        self.assertEqual(len(schedule.next),6)
        delayed=fixture.Delayed()
        for index in range(100):delayed.put(float(index),index)
        self.assertEqual(len(delayed.pending),8)
        self.assertEqual(delayed.evicted,92)
        self.assertEqual(delayed.due(100.),list(range(92,100)))
        self.assertEqual(delayed.due(100.),[])

    def test_invalid_duration_precedes_ros_guard_and_import(self):
        parser=fixture.argument_parser()
        self.assertEqual(parser.parse_args([]).seconds,120)
        for value in ['0','3601','nan','1.5']:
            with contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit):parser.parse_args(['--seconds',value])
        for seconds in [0,3601,True,1.5]:
            with patch.dict(fixture.HELPERS,{'checked_environment':lambda:self.fail('invalid input reached ROS boundary')}):
                with self.assertRaises(ValueError):fixture.publisher(seconds)

    def test_acceptance_ingress_never_conflates_final_counters_or_reactivates_retired_identity(self):
        backend=acceptance.Backend.__new__(acceptance.Backend)
        backend.reported_drops={};backend.spatial_counts={};backend.retired=set();backend.sequence={}
        backend.generation=91;backend.error=None;backend.finished=threading.Event();backend.events=queue.Queue(maxsize=16)
        backend.process=type('Live',(),{'poll':lambda self:None})()
        counters={key:0 for key in ['scans_received','pending_replaced','lookup_timeouts','tf_dynamic_batches','tf_static_batches','tf_accepted','tf_rejected','duplicate_statics','native_errors']}
        event={'event':'spatial_observation','generation':91,'subscription':2,'sequence':1,'backend_drops':4,'sample':{'counters':counters}}
        backend.events.put(event);backend.wait(lambda e:True)
        backend.events.put({'event':'drops','generation':91,'subscription':2,'backend_drops':5,'spatial_counters':counters})
        backend.wait(lambda e:True)
        self.assertEqual(backend.reported_drops[2],5)
        self.assertEqual(backend.spatial_counts[2]['pending_replaced'],0)
        backend.events.put({**event,'sequence':2,'backend_drops':5})
        with self.assertRaisesRegex(AssertionError,'Data followed final'):backend.wait(lambda e:True)


if __name__=='__main__':unittest.main()
