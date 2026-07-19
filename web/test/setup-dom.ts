import { afterEach } from "bun:test"
import { GlobalRegistrator } from "@happy-dom/global-registrator"
import { cleanup } from "@testing-library/react"

GlobalRegistrator.register()

Object.defineProperty(globalThis, "IS_REACT_ACT_ENVIRONMENT", {
  configurable: true,
  value: true,
  writable: true,
})

afterEach(() => cleanup())
