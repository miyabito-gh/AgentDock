// 送信キーの設定（AppSettings.sendKey）を、実行中の送り分けのない入力欄（side相談・編集して再送・組織の割り込み指示）へ渡す。App が提供する。
import { createContext, useContext } from "react";
import type { EnterMode } from "./sendKeys";

export const SendKeyContext = createContext<EnterMode>("enter");
export const useEnterMode = (): EnterMode => useContext(SendKeyContext);
