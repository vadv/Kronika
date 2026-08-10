declare module "kronika:registry" {
  export interface RegistryColumn {
    readonly name: string
    readonly type: string
    readonly class: string
    readonly unit: string
    readonly nullable: boolean
  }

  export interface RegistryLayout {
    readonly typeId: string
    readonly logicalName: string | null
    readonly physicalName: string
    readonly implementation: string | null
    readonly semantics: string
    readonly deprecated: boolean
    readonly sortKey: readonly string[]
    readonly identity: readonly string[]
    readonly columns: readonly RegistryColumn[]
  }

  export const registry: readonly RegistryLayout[]
}

declare module "*.css" {}
