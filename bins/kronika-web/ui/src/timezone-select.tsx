import { Select } from "@base-ui/react/select"

import type { DisplayTimeZone } from "./display-time"
import type { Translate } from "./help"

const ZONES = ["browser", "utc"] as const satisfies readonly DisplayTimeZone[]

// The native select sized its popup by OS rules and clipped the Russian labels
// inside a fixed-width box; the trigger here shrinks to the content and the
// popup is ours, so both locales render in full.
export function TimezoneSelect({ mode, setMode, t }: {
  readonly mode: DisplayTimeZone
  readonly setMode: (mode: DisplayTimeZone) => void
  readonly t: Translate
}) {
  return (
    <Select.Root<DisplayTimeZone> onValueChange={(value) => { if (value !== null) setMode(value) }} value={mode}>
      <Select.Trigger aria-label={t("timezone.switch")} className="timezone-select" data-testid="timezone-select" data-value={mode}>
        <Select.Value className="timezone-select-value">{() => t(`timezone.${mode}`)}</Select.Value>
        <Select.Icon aria-hidden="true" className="timezone-select-icon">▾</Select.Icon>
      </Select.Trigger>
      <Select.Portal>
        <Select.Positioner alignItemWithTrigger={false} className="timezone-positioner" side="bottom" sideOffset={3}>
          <Select.Popup className="timezone-popup">
            {ZONES.map((zone) => (
              <Select.Item className="timezone-option" data-testid={`timezone-option-${zone}`} key={zone} value={zone}>
                <Select.ItemText>{t(`timezone.${zone}`)}</Select.ItemText>
              </Select.Item>
            ))}
          </Select.Popup>
        </Select.Positioner>
      </Select.Portal>
    </Select.Root>
  )
}
