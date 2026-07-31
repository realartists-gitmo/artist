package mypkg

object Api:
  export internal.Helper
  export internal.utils.*

class PublicClass:
  def publicMethod(): String = "hi"

private class HiddenClass:
  def hidden(): Int = 0
