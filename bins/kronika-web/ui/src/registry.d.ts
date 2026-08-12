declare module "kronika:registry" {
  export interface RegistryLayout {
    readonly typeId: string
    readonly logicalName: string | null
    readonly identity: readonly string[]
    readonly columns: readonly string[]
  }

  export const registry: readonly RegistryLayout[]
}

declare module "*.css" {}
