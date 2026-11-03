"""Local mixed-RMW topic reproduction with per-process Cyclone tracing."""
import argparse
import importlib.util
from pathlib import Path

spec = importlib.util.spec_from_file_location('behavior', Path(__file__).with_name('ros-behavior-feasibility.py'))
behavior = importlib.util.module_from_spec(spec)
spec.loader.exec_module(behavior)
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--label', required=True)
parser.add_argument('--loopback-transports', action='store_true', help='Explicitly bind both middleware transports to loopback')
parser.add_argument('--loopback-vendor', choices=['fast','cyclone'], help='Bind only this vendor to loopback for one-variable isolation')
args = parser.parse_args()
if not args.label.replace('-', '').isalnum(): parser.error('Use an alphanumeric label')
if args.loopback_transports and args.loopback_vendor: parser.error('Choose both transports or one vendor, not both options')
suite = behavior.Suite(args.label)
config = suite.out/'cyclone-trace.xml'
interfaces = '<General><Interfaces><NetworkInterface address="127.0.0.1"/></Interfaces></General>' if args.loopback_transports or args.loopback_vendor=='cyclone' else ''
config.write_text('<CycloneDDS><Domain>' + interfaces + '<Tracing><Verbosity>finest</Verbosity><OutputFile>'
                  + str(suite.out/'cyclone-${CYCLONEDDS_PID}.log')
                  + '</OutputFile></Tracing></Domain></CycloneDDS>')
suite.env['CYCLONEDDS_URI'] = str(config)
if args.loopback_transports or args.loopback_vendor=='fast':
    fast = suite.out/'fast-loopback.xml'
    fast.write_text('''<profiles xmlns="http://www.eprosima.com/XMLSchemas/fastRTPS_Profiles">
<transport_descriptors><transport_descriptor><transport_id>fixture_loopback</transport_id><type>UDPv4</type>
<interfaceWhiteList><address>127.0.0.1</address></interfaceWhiteList></transport_descriptor></transport_descriptors>
<participant profile_name="fixture_loopback" is_default_profile="true"><rtps><useBuiltinTransports>false</useBuiltinTransports>
<userTransports><transport_id>fixture_loopback</transport_id></userTransports></rtps></participant></profiles>''')
    suite.env['FASTRTPS_DEFAULT_PROFILES_FILE'] = str(fast)
for local, peer in [(behavior.RMWS[0], behavior.RMWS[1]), (behavior.RMWS[1], behavior.RMWS[0])]:
    suite.qos(local, 'mixed-reliable', [], [], True, peer=peer)
    suite.qos(local, 'mixed-best-effort', ['--best-effort'], ['--best-effort'], True, peer=peer)
# Include diagnostic configuration and all trace files in each reproducible record.
import json
for row in suite.rows:
    row['diagnostic_environment'] = {'CYCLONEDDS_URI': str(config)}
    if args.loopback_transports or args.loopback_vendor=='fast':
        row['diagnostic_environment']['FASTRTPS_DEFAULT_PROFILES_FILE'] = str(fast)
        row['evidence'].append(str(fast.relative_to(behavior.RAW)))
    row['evidence'] += [str(p.relative_to(behavior.RAW)) for p in [config, *suite.out.glob('cyclone-*.log')]]
suite.report.write_text(json.dumps(suite.rows, indent=2)+'\n')
raise SystemExit(0 if all(row['outcome']=='passed' for row in suite.rows) else 1)
