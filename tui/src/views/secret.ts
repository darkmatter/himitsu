import {
  Box, Text,
  type CliRenderer,
} from "@opentui/core"
import { colors } from "../theme"
import * as himitsu from "../himitsu"

export function SecretView(
  renderer: CliRenderer,
  remote: string | undefined,
  env: string,
  onAction: (action: string, data?: any) => void,
) {
  const label = remote ?? "local"
  const secrets = himitsu.listSecrets(env, remote)

  const rows = secrets.map((key) => {
    let value: string
    try {
      value = himitsu.getSecret(env, key, remote) ?? "(null)"
    } catch {
      value = "(decryption failed)"
    }
    return Box(
      { flexDirection: "row", marginLeft: 2 },
      Text({ content: key.padEnd(28), fg: colors.cyan }),
      Text({ content: value, fg: colors.fg }),
    )
  })

  return Box(
    { flexDirection: "column", flexGrow: 1 },
    Box(
      { flexDirection: "row", marginBottom: 1 },
      Text({ content: ` ${label}`, fg: colors.accent }),
      Text({ content: ` / ${env}`, fg: colors.yellow }),
    ),
    Box(
      { flexDirection: "column", marginLeft: 2, marginBottom: 1 },
      Text({ content: "KEY                         VALUE", fg: colors.fgMuted }),
    ),
    ...rows,
    ...(rows.length === 0
      ? [Text({ content: "  (no secrets)", fg: colors.fgDim, marginLeft: 2 })]
      : []),
    Text({ content: "" }),
    Text({ content: "  esc  back    /  search", fg: colors.fgDim }),
  )
}
