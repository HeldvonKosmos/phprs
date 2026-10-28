/** Measurement tool (BENCH3, THESIS1): the user of a hydrated note. */
export class UserView {
  constructor(public id: number, public name: string, public email: string) {}
  toArray() {
    return { id: this.id, name: this.name, email: this.email }
  }
}

/** Measurement tool (BENCH3, THESIS1): a note hydrated from a raw row, with its user. */
export class NoteView {
  constructor(public id: number, public title: string, public body: string | null, public done: boolean, public priority: number, public createdAt: string, public user: UserView) {}
  static fromRow(r: any) {
    return new NoteView(r.id, r.title, r.body, r.done === 1, r.priority, r.created_at, new UserView(r.user_id, r.user_name, r.user_email))
  }
  toArray() {
    return { id: this.id, title: this.title, body: this.body, done: this.done, priority: this.priority, created_at: this.createdAt, user: this.user.toArray() }
  }
}
