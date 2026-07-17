import { StrictMode } from "react"
import { createRoot } from "react-dom/client"

import "./index.css"
import { ThemeProvider } from "@/components/theme-provider.tsx"
import App from "./App.tsx"

const rootElement = document.getElementById("root")

if (!rootElement) {
  throw new Error("Unable to find the root application element")
}

createRoot(rootElement).render(
  <StrictMode>
    <ThemeProvider defaultTheme="system" storageKey="word-arena-theme">
      <App />
    </ThemeProvider>
  </StrictMode>
)
