import stored from "../../product-semantics.json" with { type: "json" }

export type SemanticOrigin = "recorded" | "kronika_derived" | "accepted_presentation"
export type SemanticUnit = "percent" | "milliseconds" | "milliseconds_per_call" | "samples" | "sampling_intervals"
export type ValueTone = "good" | "warning" | "critical" | "inactive"
export type VacuumRisk = "ordinary" | "heavy" | "dangerous"
export type EventTier = "critical" | "notable" | "routine"

export interface SemanticThreshold {
  readonly operator: "lt" | "gte"
  readonly value: number
  readonly tone: ValueTone
}

export interface ExpectedBand {
  readonly min_inclusive: number | null
  readonly max_exclusive: number | null
}

interface NumericValueTonePolicy {
  readonly kind: "numeric_value_tone"
  readonly field: string
  readonly active_client_only: boolean
}

interface TextValueTonePolicy {
  readonly kind: "text_value_tone"
  readonly field: string
  readonly values: Readonly<Record<string, ValueTone>>
  readonly ascii_values: boolean
  readonly nonempty_tone: ValueTone | null
}

interface VacuumMovement {
  readonly phase: string
  readonly field: string
  readonly unavailable_type_ids: readonly string[]
}

interface RelationState {
  readonly valid: boolean
  readonly ready: boolean | null
  readonly severity: number
}

interface EventTierPolicy {
  readonly kind: "event_tier"
  readonly section: string
  readonly discriminator: string | null
  readonly tiers: readonly EventTier[]
  readonly fallback: EventTier
  readonly provenance: SemanticOrigin
}

export type SemanticPolicy =
  | NumericValueTonePolicy
  | TextValueTonePolicy
  | { readonly kind: "rate_zero_tone"; readonly tone: ValueTone }
  | { readonly kind: "vacuum_episode"; readonly adjacency_factor: number }
  | { readonly kind: "vacuum_no_movement"; readonly samples: number; readonly phases: readonly VacuumMovement[] }
  | { readonly kind: "vacuum_risk"; readonly default: VacuumRisk; readonly order: readonly VacuumRisk[]; readonly phases: Readonly<Record<string, VacuumRisk>> }
  | { readonly kind: "relation_severity"; readonly states: readonly RelationState[] }
  | { readonly kind: "event_tier_order"; readonly tiers: readonly EventTier[] }
  | EventTierPolicy

export interface SemanticDefinition {
  readonly id: string
  readonly origin: SemanticOrigin
  readonly unit: SemanticUnit | null
  readonly formula: string | null
  readonly operands: readonly string[]
  readonly thresholds: readonly SemanticThreshold[]
  readonly expected_band: ExpectedBand | null
  readonly policy: SemanticPolicy
}

type PolicyKind = SemanticPolicy["kind"]
export type SemanticOf<K extends PolicyKind> = SemanticDefinition & {
  readonly policy: Extract<SemanticPolicy, { readonly kind: K }>
}

export const productSemantics = stored.definitions as readonly SemanticDefinition[]

export function semanticsOf<K extends PolicyKind>(kind: K): readonly SemanticOf<K>[] {
  return productSemantics.filter((definition): definition is SemanticOf<K> => definition.policy.kind === kind)
}

export function semantic<K extends PolicyKind>(id: string, kind: K): SemanticOf<K> {
  const definition = productSemantics.find((candidate) => candidate.id === id)
  if (definition === undefined || definition.policy.kind !== kind) {
    throw new Error(`invalid product semantic ${id}`)
  }
  return definition as SemanticOf<K>
}
