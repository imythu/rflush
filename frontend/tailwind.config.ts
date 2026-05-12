import type { Config } from "tailwindcss";

export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        background: "#f8f3ff",
        foreground: "#20173d",
        card: "rgba(255, 252, 255, 0.88)",
        border: "rgba(126, 96, 194, 0.18)",
        input: "rgba(255, 252, 255, 0.74)",
        primary: "#7d5cff",
        "primary-foreground": "#ffffff",
        secondary: "#eee5ff",
        "secondary-foreground": "#4a347f",
        muted: "#6d6289",
        accent: "#f4edff",
        destructive: "#d83a57",
        ring: "#8c6cff",
        surface: "#fffaff",
        "surface-container": "rgba(245, 238, 255, 0.72)",
        blossom: "#ff7da8",
        night: "#1a1635",
        jade: "#4db6ac"
      },
      boxShadow: {
        card: "0 18px 50px rgba(68, 48, 126, 0.10), 0 1px 0 rgba(255, 255, 255, 0.92) inset",
        glow: "0 20px 60px rgba(125, 92, 255, 0.22)",
      },
      borderRadius: {
        xl: "1.5rem",
        "3xl": "1.75rem",
      },
    },
  },
  plugins: [],
} satisfies Config;
