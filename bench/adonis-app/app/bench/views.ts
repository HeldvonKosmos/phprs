/** Measurement tool (BENCH3, THESIS1): the user of a hydrated note. */
export class UserView {
  constructor(
    public id: number,
    public name: string,
    public email: string
  ) {}

  toArray() {
    return { id: this.id, name: this.name, email: this.email }
  }
}

/** Measurement tool (BENCH3, THESIS1): a note hydrated from a raw row, with its user. */
export class NoteView {
  constructor(
    public id: number,
    public title: string,
    public body: string | null,
    public done: boolean,
    public priority: number,
    public createdAt: string,
    public user: UserView
  ) {}

  static fromRow(row: any): NoteView {
    return new NoteView(row.id, row.title, row.body, row.done === 1, row.priority, row.created_at, new UserView(row.user_id, row.user_name, row.user_email))
  }

  toArray() {
    return { id: this.id, title: this.title, body: this.body, done: this.done, priority: this.priority, created_at: this.createdAt, user: this.user.toArray() }
  }
}
