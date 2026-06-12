// VRule.tsx — a 1 px vertical divider between toolbar groups.

import type { JSX } from "solid-js";

export interface VRuleProps {
  o?: number;
}

export function VRule(props: VRuleProps): JSX.Element {
  const o = (): number => props.o ?? 0.09;
  return (
    <span
      style={{
        width: "1px",
        "align-self": "stretch",
        margin: "8px 0",
        background: `rgba(255,255,255,${o()})`,
      }}
    />
  );
}
