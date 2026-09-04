import type { Cell, DataRow } from "./api"
import { rawText, value } from "./model"

export interface CgroupMountAssociation {
  readonly infrastructure: boolean | null
  readonly mountPoint: string
  readonly root: string | null
  readonly source: string | null
}

export interface CgroupDevicePresentation {
  readonly associations: readonly CgroupMountAssociation[]
  readonly device: string | null
  readonly id: string
  readonly preferredMounts: readonly string[]
  readonly source: string | null
}

export function cgroupDevicePresentation(
  row: DataRow,
  mounts: readonly DataRow[],
  devices: readonly DataRow[],
): CgroupDevicePresentation | null {
  const id = exactDeviceId(row)
  if (id === null) return null
  const exact = (candidate: DataRow) => exactDeviceId(candidate) === id
  const associations = mounts.filter(exact).map((mount): CgroupMountAssociation | null => {
    const mountPoint = rawText(value(mount, "mount_point"))
    if (mountPoint === null) return null
    return {
      infrastructure: storedBoolean(value(mount, "is_k8s_infra")),
      mountPoint,
      root: rawText(value(mount, "root")),
      source: rawText(value(mount, "source")),
    }
  }).filter((association): association is CgroupMountAssociation => association !== null)
    .filter((association, index, all) => all.findIndex((candidate) => associationKey(candidate) === associationKey(association)) === index)
    .sort(compareAssociations)
  const names = [...new Set(devices.filter(exact).map((device) => rawText(value(device, "device"))).filter((name): name is string => name !== null))].sort(compareText)
  const preferred = associations.filter(({ infrastructure }) => infrastructure !== true)
  return {
    associations,
    device: names.length === 1 ? names[0]! : null,
    id,
    preferredMounts: [...new Set(preferred.map(({ mountPoint }) => mountPoint))],
    source: preferred.find(({ source }) => source !== null)?.source ?? associations.find(({ source }) => source !== null)?.source ?? null,
  }
}

export function cgroupDevicePrimary(presentation: CgroupDevicePresentation): string | null {
  const mount = presentation.preferredMounts[0]
  if (mount === undefined) return presentation.device
  const source = presentation.source === null
    ? null
    : presentation.source.split("/").filter((part) => part !== "").at(-1) ?? presentation.source
  const device = source ?? presentation.device
  return device === null ? mount : `${device} → ${mount}`
}

export function exactDeviceId(row: DataRow): string | null {
  const major = rawText(value(row, "major"))
  const minor = rawText(value(row, "minor"))
  return major === null || minor === null ? null : `${major}:${minor}`
}

function associationKey(association: CgroupMountAssociation): string {
  return [association.mountPoint, association.root ?? "", association.source ?? "", String(association.infrastructure)].join("\u0000")
}

function compareAssociations(left: CgroupMountAssociation, right: CgroupMountAssociation): number {
  const leftRank = left.infrastructure === true ? 1 : 0
  const rightRank = right.infrastructure === true ? 1 : 0
  return leftRank - rightRank
    || left.mountPoint.length - right.mountPoint.length
    || compareText(left.mountPoint, right.mountPoint)
    || compareText(left.root ?? "", right.root ?? "")
    || compareText(left.source ?? "", right.source ?? "")
}

function compareText(left: string, right: string): number {
  return left < right ? -1 : left > right ? 1 : 0
}

function storedBoolean(cell: Cell): boolean | null {
  if (typeof cell === "boolean") return cell
  if (typeof cell === "number") return cell !== 0
  if (typeof cell === "string" && ["true", "1"].includes(cell.toLowerCase())) return true
  if (typeof cell === "string" && ["false", "0"].includes(cell.toLowerCase())) return false
  return null
}
