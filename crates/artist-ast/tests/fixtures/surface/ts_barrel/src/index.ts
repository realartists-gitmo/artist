export { Client } from './client';
export { Util as Helper } from './util';
export * from './types';

export class Direct {
  greet(): string { return 'hi'; }
}

const _internal = 1;
export { _internal };
