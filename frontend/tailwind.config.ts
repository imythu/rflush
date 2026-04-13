import type { Config } from "tailwindcss";

export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        background: "#f7f2fa",
        foreground: "#1d1b20",
        card: "#fffbfe",
        border: "#e7e0ec",
        input: "#fffbfe",
        primary: "#6750a4",
        "primary-foreground": "#ffffff",
        secondary: "#e8def8",
        "secondary-foreground": "#4a4458",
        muted: "#625b71",
        accent: "#f3edf7",
        destructive: "#ba1a1a",
        ring: "#6750a4",
        surface: "#fef7ff",
        "surface-container": "#f3edf7"
      },
      boxShadow: {
        card: "0 1px 2px rgba(16, 24, 40, 0.08), 0 8px 24px rgba(103, 80, 164, 0.08)",
      },
      borderRadius: {
        xl: "1.5rem",
      },
    },
  },
  plugins: [],
} satisfies Config;
