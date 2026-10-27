export function charge(amount) {
  return amount + fee(amount);
}

function fee(amount) {
  // Flat 2% fee, rounded up to the cent.
  return Math.ceil(amount * 2) / 100;
}

export function todoList(items) {
  const todo_items = items.filter((i) => !i.done);
  console.error('refund failed');
  return todo_items;
}
