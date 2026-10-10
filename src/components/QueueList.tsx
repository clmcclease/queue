import type { QueueItem } from "../queue";

export default function QueueList({ items }: { items: QueueItem[] }) {
  if (items.length === 0) {
    return <p className="mt-16 text-center text-sm text-neutral-500 dark:text-neutral-400">Queue is empty.</p>;
  }
  return (
    <ul aria-label="Queue" className="mt-4 divide-y divide-neutral-200 dark:divide-neutral-800">
      {items.map((item) => (
        <li key={item.id} className="truncate py-2 font-mono text-sm text-neutral-800 dark:text-neutral-200" title={item.url}>
          {item.url}
        </li>
      ))}
    </ul>
  );
}
