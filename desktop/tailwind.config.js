/** @type {import('tailwindcss').Config} */
export default {
  darkMode: ["class"],
  content: [
    "./index.html",
    "./src/**/*.{vue,js,ts,jsx,tsx}",
  ],
  screens: {
    xs: "400px",
    sm: "640px",
    md: "768px",
    lg: "1024px",
    xl: "1280px",
    "2xl": "1536px",
  },
  theme: {
    extend: {
      colors: {
        border: "hsl(var(--border))",
        input: "hsl(var(--input))",
        ring: "hsl(var(--ring))",
        background: "hsl(var(--background))",
        foreground: "hsl(var(--foreground))",
        primary: {
          DEFAULT: "hsl(var(--primary))",
          foreground: "hsl(var(--primary-foreground))",
        },
        secondary: {
          DEFAULT: "hsl(var(--secondary))",
          foreground: "hsl(var(--secondary-foreground))",
        },
        destructive: {
          DEFAULT: "hsl(var(--destructive))",
          foreground: "hsl(var(--destructive-foreground))",
        },
        muted: {
          DEFAULT: "hsl(var(--muted))",
          foreground: "hsl(var(--muted-foreground))",
        },
        accent: {
          DEFAULT: "hsl(var(--accent))",
          foreground: "hsl(var(--accent-foreground))",
        },
        popover: {
          DEFAULT: "hsl(var(--popover))",
          foreground: "hsl(var(--popover-foreground))",
        },
        card: {
          DEFAULT: "hsl(var(--card))",
          foreground: "hsl(var(--card-foreground))",
        },
        sidebar: {
          DEFAULT: "hsl(var(--sidebar))",
          foreground: "hsl(var(--sidebar-foreground))",
          accent: "hsl(var(--sidebar-accent))",
          "accent-foreground": "hsl(var(--sidebar-accent-foreground))",
          border: "hsl(var(--sidebar-border))",
        },
      },
      borderRadius: {
        "2xl": "14px",
        xl: "12px",
        lg: "var(--radius)",
        md: "calc(var(--radius) - 2px)",
        sm: "calc(var(--radius) - 4px)",
        xs: "4px",
        pill: "999px",
      },
      fontFamily: {
        sans: [
          "Inter",
          "ui-sans-serif",
          "-apple-system",
          "BlinkMacSystemFont",
          "Segoe UI",
          "sans-serif",
        ],
        mono: [
          "SFMono-Regular",
          "SF Mono",
          "Cascadia Code",
          "Roboto Mono",
          "ui-monospace",
          "monospace",
        ],
      },
      fontSize: {
        "ui-xs": ["11px", "16px"],
        "ui-sm": ["12px", "18px"],
        "ui-md": ["13px", "20px"],
        "body": ["14px", "21px"],
        "title-sm": ["15px", "22px"],
        "title-md": ["18px", "26px"],
        "display": ["24px", "32px"],
      },
      spacing: {
        "1.5": "6px",
        "2.5": "10px",
        "3.5": "14px",
      },
      transitionDuration: {
        "100": "100ms",
        "140": "140ms",
        "180": "180ms",
        "220": "220ms",
      },
      transitionTimingFunction: {
        "grok": "cubic-bezier(0.2, 0, 0, 1)",
      },
      boxShadow: {
        popover: "0 8px 24px rgb(0 0 0 / 0.12), 0 2px 8px rgb(0 0 0 / 0.08)",
        dialog: "0 20px 48px rgb(0 0 0 / 0.18), 0 6px 18px rgb(0 0 0 / 0.10)",
      },
      keyframes: {
        "fade-in": {
          from: { opacity: "0" },
          to: { opacity: "1" },
        },
        "scale-in": {
          from: { opacity: "0", transform: "scale(0.98)" },
          to: { opacity: "1", transform: "scale(1)" },
        },
        "slide-in-right": {
          from: { transform: "translateX(100%)" },
          to: { transform: "translateX(0)" },
        },
      },
      animation: {
        "fade-in": "fade-in 140ms cubic-bezier(0.2, 0, 0, 1)",
        "scale-in": "scale-in 180ms cubic-bezier(0.2, 0, 0, 1)",
        "slide-in-right": "slide-in-right 220ms cubic-bezier(0.2, 0, 0, 1)",
      },
    },
  },
  plugins: [require("tailwindcss-animate")],
};
