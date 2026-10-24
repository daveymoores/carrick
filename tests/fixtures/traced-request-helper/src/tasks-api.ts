import { send } from "./traced";
import { sendPlain } from "./plain";

export const tasksApi = {
  setStatus: async (id: string, status: string) => {
    const response = await send(`/v1/tasks/${id}/status`, { method: "PUT", body: JSON.stringify({ status }) });
    return response.json();
  },
  load: async (id: string) => {
    const response = await send(`/v1/tasks/${id}`);
    return response.json();
  },
  archive: async (id: string) => {
    const response = await sendPlain(`/v1/tasks/${id}/archive`, { method: "PUT" });
    return response.json();
  },
};
