import subprocess
import sys
from pathlib import Path
if sys.platform == 'darwin':
    prefix = Path(__file__).resolve().parents[1]/'artifacts/fixture-install'
    runtime = Path(__file__).resolve().parents[1]/'.pixi/envs/default/lib'
    for p in list(prefix.rglob('*.dylib')) + list(prefix.rglob('*.so')):
        load = subprocess.check_output(['otool','-l',str(p)],text=True)
        for path in [str(prefix/'lib'),str(runtime)]:
            if f'path {path} (offset' not in load:
                subprocess.run(['install_name_tool','-add_rpath',path,str(p)],check=True,capture_output=True)
        subprocess.run(['codesign','--force','--sign','-',str(p)],check=True,capture_output=True)
