# Image size, memory and start time

Measured on the published Linux x86_64 image of release `v0.1.0-beta.8` (source commit `159fbab36132be34341948bac2a442dfd0d4cd04`):

`ghcr.io/nicoleman0/sylux@sha256:9be7fa0786ab2e497637179d7679d21c9d1256679cbd5128b9c6df45780ab171`

These are measurements of one release on one small host. They are not capacity, scaling or durability claims. Other hosts, file counts and sizes will differ.

## Results

| Figure | Measured | Workload |
| --- | ---: | --- |
| Image size, compressed layers | 36.9 MB (35.2 MiB) | Registry manifest. |
| Image size, unpacked | 95.3 MB (90.9 MiB) | Docker overlay2 storage. Other image stores report different sizes. |
| Idle memory (RSS) | 49.2 MiB | Server process 30 seconds after first sign-in, with no requests. Range over 3 runs: 49.1 to 49.3 MiB. |
| Peak memory (RSS) during workload | 61.5 MiB | Four parallel clients upload 40 files of 4 MiB (160 MiB), list the root folder every 50 ms meanwhile, then download and checksum every file. Highest of 3 runs; the others were 60.4 MiB. Peak is the kernel's high-water mark for the process. |
| Start to ready | median 559 ms (112 to 1,073 ms) | Container start until `/readyz` answers, on an initialised, empty root. 15 starts over 3 runs. Includes Docker's container start. |

Each workload run took 5.7 to 6.1 seconds. Memory after the workload stayed within 0.2 MiB of the peak, so the figure is not a leak test. Container cgroup memory (about 222 MiB after the workload) includes the page cache of the files written and read, which the kernel can reclaim. It is not a measure of what the server needs.

## Host and method

- Host: virtualised Linux 7.0 x86_64 guest, Intel Core i5-9500T at 2.2 GHz, 2 vCPUs, 1 GiB RAM, ext4, Docker Engine 26.1.5, cgroup v2. Other services ran on the machine; its load average was about 0.4 when checked beforehand.
- Measured on 2026-11-18. The image was pulled by digest and its platform checked as `linux/amd64`.
- The container uses the options in `deploy/docker/compose.yaml`: host networking, UID 10001, read-only root, no capabilities, `no-new-privileges` and an init process. Data is a disposable bind-mounted directory on the host's ext4 disk, deleted afterward. TLS uses a throwaway self-signed certificate.
- Memory is `VmRSS` and `VmHWM` of the `sylux` process from `/proc`, sampled every 100 ms during the workload. Start time is the interval from Docker's recorded container start to the first successful `/readyz`, polled every 2 ms.
- Not measured: other architectures, large libraries, previews, archives, idle memory after long uptime, and any other product.

## Reproduce

On a Linux x86_64 host with Docker (and Buildx), Node 22 or newer, `openssl` and cgroup v2, run as root:

```sh
scripts/measure-image-footprint.sh ghcr.io/nicoleman0/sylux@sha256:<digest>
```

Use the digest of the release image you want to measure. The script refuses a tag and any host that is not x86_64. It prints JSON with the host, the image sizes, each start time and the memory samples. Set `RUNS`, `IDLE_SECONDS`, `FILES`, `FILE_MIB` or `WORKERS` to change the workload; the table above used the defaults (5 starts, 30 seconds, 40 files, 4 MiB, 4 workers). `scripts/measure-image-footprint.mjs` is the client it drives.

For in-process fixture timings from a source checkout, see [file workflow measurements](performance.md).
