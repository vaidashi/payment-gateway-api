import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRoot } from "react-dom/client";
import "./styles.css";

const queryClient = new QueryClient();

function RuntimeShell() {
  return (
    <main>
      <p className="eyebrow">Local development runtime</p>
      <h1>Mock Food Ordering</h1>
      <p>The ordering experience arrives in later implementation units.</p>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(
  <QueryClientProvider client={queryClient}><RuntimeShell /></QueryClientProvider>,
);
