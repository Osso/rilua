"""Run through pyrun_eval: exec(fs.read('scripts/bench-vm-capabilities.pyrun.py')).
Build both interpreter Criterion executables first; see the proof ledger.
Set ctx.vm_bench_phase = 'after' and ctx.vm_bench_branch to a new executable
for a subsequent comparison. No compilation runs during measurements.
"""
import json
import random
import statistics

phase = getattr(ctx, 'vm_bench_phase', 'before')
branch = getattr(ctx, 'vm_bench_branch',
                 'target/bench-before/release/deps/interpreter-acc3f86cd9dedfee')
base = 'target/bench-base/release/deps/interpreter-acc3f86cd9dedfee'
cases = ['end_to_end/coroutine_cycle', 'vm_execution/closures_100',
         'vm_execution/table_build_1k', 'vm_execution/string_concat_100',
         'end_to_end/compile_and_run']
records = []
load_start = cli.uptime().capture().run().stdout
rng = random.Random(7608)
for round_index in range(20):
    order = cases.copy()
    rng.shuffle(order)
    for case in order:
        configurations = [('base', base), ('branch', branch)]
        if round_index % 2:
            configurations.reverse()
        for label, executable in configurations:
            baseline = f'vm-{phase}-{label}-{round_index}'
            command = cli.command('taskset', '-c', '18', executable, case,
                                  '--bench', '--sample-size', '20',
                                  '--measurement-time', '1', '--warm-up-time', '0.1',
                                  '--nresamples', '100', '--noplot',
                                  '--save-baseline', baseline)
            result = command.capture().run()
            fs.write(f'target/{baseline}-{case.replace("/", "-")}.out',
                     result.stdout + result.stderr)
            sample_dir = f'target/criterion/{case}/{baseline}'
            estimates = json.loads(fs.read(f'{sample_dir}/estimates.json'))
            samples = json.loads(fs.read(f'{sample_dir}/sample.json'))
            records.append({'round': round_index, 'case': case, 'label': label,
                            'median_ns': estimates['median']['point_estimate'],
                            'samples': samples})
    fs.write_json(f'target/vm-benchmark-{phase}.json', records)
    print(f'{phase}: round {round_index + 1}/20 complete', flush=True)
summary = {'load_start': load_start, 'load_end': cli.uptime().capture().run().stdout,
           'cases': {}}
for case in cases:
    summary['cases'][case] = {}
    for label in ('base', 'branch'):
        values = [r['median_ns'] / 1000 for r in records
                  if r['case'] == case and r['label'] == label]
        quartiles = statistics.quantiles(values, n=4)
        summary['cases'][case][label] = {
            'median_us': statistics.median(values),
            'p25_us': quartiles[0], 'p75_us': quartiles[2],
            'min_us': min(values), 'max_us': max(values)}
fs.write_json(f'target/vm-benchmark-{phase}-summary.json', summary)
print(json.dumps(summary, indent=2))
