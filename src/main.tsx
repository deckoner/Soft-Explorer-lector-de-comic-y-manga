import { createRoot } from "react-dom/client";
import App from "./ui/App";
// Las fuentes se incluyen en la aplicación para no depender de una CDN ni de Internet.
import "@fontsource/dm-sans/400.css";
import "@fontsource/dm-sans/500.css";
import "@fontsource/dm-sans/600.css";
import "@fontsource/dm-sans/700.css";
import "@fontsource/bricolage-grotesque/500.css";
import "@fontsource/bricolage-grotesque/600.css";
import "@fontsource/bricolage-grotesque/700.css";
import "./ui/styles.css";

createRoot(document.getElementById("root")!).render(<App />);
