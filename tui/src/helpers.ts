import type { BoxRenderable, Renderable } from "@opentui/core"

/** Remove and destroy all children from a container renderable. */
export function clearChildren(container: BoxRenderable) {
  for (const child of container.getChildren()) {
    (child as Renderable).destroy()
  }
}
