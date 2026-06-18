# Host Tuning

This page groups host-level tuning guidance relevant to production Spooky deployments.

## Host Priorities

- sufficient UDP buffer sizing
- sufficient file descriptor limits
- stable CPU scheduling under multi-worker load
- predictable network path MTU
- minimal interference from unrelated noisy workloads

## Linux Tuning Areas

Important areas to validate:

- receive and send socket buffer sizes
- device backlog and packet budget
- file descriptor ceilings
- capability model for privileged ports
- conntrack impact, if present in the environment

## Built-In Project Guidance

The repository already includes:

- production guidance in [Production Deployment](../deployment/production.md)
- a Linux sysctl helper in `scripts/sysctl-linux-network-tuning.sh`

Use those as a baseline, then tune with real traffic and host telemetry.

## Practical Advice

- do not treat aggressive sysctl values as universally correct
- validate tuning with the same traffic pattern you expect in production
- keep cert, config, and log path permissions minimal
- isolate the process from unrelated noisy co-located workloads where possible
- verify privileged-port bind strategy before rollout
