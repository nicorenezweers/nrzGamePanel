// Panel-eigene Stile, ergänzend zu style/nrzui.rs.

// Konsole und Pixelgrafik.
pub const PANEL: &str = r#"
/* Konsole: Zeilen umbrechen, Monospace */
.console-line {
  white-space: pre-wrap;
  word-break: break-word;
}

/* Minecraft-Item-Pixelgrafik nicht weichzeichnen */
.pixelated {
  image-rendering: pixelated;
}
"#;
