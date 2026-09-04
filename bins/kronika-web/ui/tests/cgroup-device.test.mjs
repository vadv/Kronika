import assert from "node:assert/strict"
import test from "node:test"

import { importModule, registryPlugin } from "./import-module.mjs"

const devices = await importModule(
  'export { cgroupDevicePresentation, cgroupDevicePrimary, exactDeviceId } from "../src/cgroup-device.ts"',
  { plugins: [registryPlugin([])] },
)

const row = (logicalName, ordinal, values) => ({
  logicalName,
  ordinal,
  segmentId: "frozen-zms",
  timestamp: 1_788_523_170_023_568,
  typeId: logicalName,
  values,
})

test("cgroup device labels use only exact major:minor mount and disk metadata", () => {
  const io = row("os_cgroup_io", "252:0", { cgroup_path: "/", major: 252, minor: 0, scope: 3 })
  const mounts = [
    row("os_mountinfo", "root", { major: 252, minor: 0, mount_point: "/volumes/kronika-pages-08c6f4d0-data-2/_data", root: "/", source: "/dev/mapper/data-docker", is_k8s_infra: false, scope: 0 }),
    row("os_mountinfo", "data", { major: 252, minor: 0, mount_point: "/var/lib/kronika/data", root: "/volumes/kronika-pages-08c6f4d0-data-2/_data", source: "/dev/mapper/data-docker", is_k8s_infra: false, scope: 0 }),
    row("os_mountinfo", "hosts", { major: 252, minor: 0, mount_point: "/etc/hosts", root: "/containers/id/hosts", source: "/dev/mapper/data-docker", is_k8s_infra: "true", scope: 0 }),
    row("os_mountinfo", "other", { major: 259, minor: 0, mount_point: "/wrong", source: "/dev/nvme0n1", is_k8s_infra: false, scope: 0 }),
  ]
  const diskRows = [
    row("os_diskstats", "dm-0", { major: 252, minor: 0, device: "dm-0", scope: 0 }),
    row("os_diskstats", "partition", { major: 259, minor: 4, device: "nvme0n1p4", scope: 0 }),
  ]

  const presentation = devices.cgroupDevicePresentation(io, mounts, diskRows)
  assert.equal(presentation.id, "252:0")
  assert.equal(presentation.device, "dm-0")
  assert.equal(presentation.source, "/dev/mapper/data-docker")
  assert.deepEqual(presentation.preferredMounts, ["/var/lib/kronika/data", "/volumes/kronika-pages-08c6f4d0-data-2/_data"])
  assert.equal(devices.cgroupDevicePrimary(presentation), "data-docker → /var/lib/kronika/data")
  assert.equal(presentation.associations.some(({ mountPoint }) => mountPoint === "/etc/hosts"), true)

  const opaque = devices.cgroupDevicePresentation(
    row("os_cgroup_io", "259:0", { cgroup_path: "/", major: 259, minor: 0, scope: 3 }),
    mounts,
    diskRows,
  )
  assert.deepEqual(opaque, { associations: [], device: null, id: "259:0", preferredMounts: [], source: null })
  assert.equal(devices.cgroupDevicePrimary(opaque), null)
})

test("ambiguous exact disk names stay opaque", () => {
  const io = row("os_cgroup_io", "7:0", { cgroup_path: "/", major: 7, minor: 0 })
  const presentation = devices.cgroupDevicePresentation(io, [], [
    row("os_diskstats", "a", { major: 7, minor: 0, device: "loop0" }),
    row("os_diskstats", "b", { major: 7, minor: 0, device: "loop-other" }),
  ])
  assert.equal(presentation.device, null)
  assert.equal(devices.cgroupDevicePrimary(presentation), null)
})
