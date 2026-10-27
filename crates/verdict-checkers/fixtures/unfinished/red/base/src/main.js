import { charge } from './pay';

export function checkout(total) {
  return charge(total);
}
