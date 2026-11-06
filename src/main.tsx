import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { PetWindow } from "./ui/pet-window";
import "./styles.css";

const root = document.getElementById("root");

if (!root) {
  throw new Error("找不到应用根节点。");
}

createRoot(root).render(
  <StrictMode>
    {new URLSearchParams(window.location.search).get("window") === "pet" ? <PetWindow /> : <App />}
  </StrictMode>,
);
